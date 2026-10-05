#!/usr/bin/env bash
# Builds the throwaway repository the demo tapes record: a few commits, a branch, a stash,
# and uncommitted edits. Usage: setup.sh <dir>
set -euo pipefail
dir=${1:?usage: setup.sh <dir>}
rm -rf "$dir"
mkdir -p "$dir/src" "$dir/.github/workflows"
cd "$dir"
git init -q -b main
git config user.name "Ada Lovelace"
git config user.email ada@example.com
git config commit.gpgsign false
commit() { git add -A && GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" git commit -qm "$2"; }

cat > Cargo.toml <<'T'
[package]
name = "forecast"
version = "0.3.0"
edition = "2024"

[dependencies]
serde = { version = "1", features = ["derive"] }
ureq = "3"
T
cat > src/main.rs <<'T'
mod api;
mod config;

use config::Config;

fn main() {
    let config = Config::load().unwrap_or_default();
    let city = std::env::args().nth(1).unwrap_or(config.city.clone());
    match api::today(&config, &city) {
        Ok(day) => println!("{city}: {} °C, {}", day.temp, day.sky),
        Err(err) => eprintln!("forecast: {err}"),
    }
}
T
commit "2026-09-01T10:00:00" "feat: print today's weather"

cat > src/config.rs <<'T'
use std::path::PathBuf;

use serde::Deserialize;

/// Settings read from `~/.config/forecast.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// City used when none is given on the command line.
    pub city: String,
    /// Base URL of the weather API.
    pub endpoint: String,
    /// Seconds before a request gives up.
    pub timeout: u64,
    /// Celsius or Fahrenheit.
    pub units: Units,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    Celsius,
    Fahrenheit,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            city: "London".into(),
            endpoint: "https://api.example.com/v1".into(),
            timeout: 10,
            units: Units::Celsius,
        }
    }
}

impl Config {
    /// The config file's path, if the home directory is known.
    pub fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".config/forecast.toml"))
    }

    /// Reads the config file; `None` when it is missing.
    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()?).ok()?;
        toml::from_str(&text).ok()
    }

    /// Converts a Celsius reading to the configured units.
    pub fn convert(&self, celsius: f64) -> f64 {
        match self.units {
            Units::Celsius => celsius,
            Units::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        }
    }

    /// The label shown after a temperature.
    pub fn suffix(&self) -> &'static str {
        match self.units {
            Units::Celsius => "°C",
            Units::Fahrenheit => "°F",
        }
    }
}
T
cat > src/api.rs <<'T'
use serde::Deserialize;

use crate::config::Config;

#[derive(Debug, Deserialize)]
pub struct Day {
    pub temp: f64,
    pub sky: String,
}

/// Today's weather in `city`.
pub fn today(config: &Config, city: &str) -> Result<Day, ureq::Error> {
    let url = format!("{}/today?city={city}", config.endpoint);
    let day: Day = ureq::get(&url).call()?.body_mut().read_json()?;
    Ok(day)
}
T
commit "2026-09-03T14:20:00" "feat: config file and units"

cat > .github/workflows/ci.yml <<'T'
name: CI
on: [push]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
T
cat > README.md <<'T'
# forecast

Prints today's weather for a city.

    forecast Paris
T
commit "2026-09-05T09:45:00" "ci: run tests on push"

git switch -q -c retry-requests
sed -i '' 's/let day: Day = ureq::get(&url).call()?/let day: Day = retry(3, || ureq::get(\&url).call())?/' src/api.rs
commit "2026-09-08T16:10:00" "feat: retry failed requests"
git switch -q main

echo "Set \`city\` in ~/.config/forecast.toml to change the default." >> README.md
git stash push -q -m "readme: document the config file"

cat > Cargo.toml <<'T'
[package]
name = "forecast"
version = "0.4.0"
edition = "2024"

[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.9"
ureq = "3"
T
sed -i '' -e 's|/// Seconds before a request gives up.|/// Seconds before a request gives up; 0 waits forever.|' \
    -e 's|            timeout: 10,|            timeout: 30,|' \
    -e 's|            Units::Fahrenheit => celsius \* 9.0 / 5.0 + 32.0,|            Units::Fahrenheit => celsius.mul_add(1.8, 32.0),|' src/config.rs
cat > src/api.rs <<'T'
use std::time::Duration;

use serde::Deserialize;

use crate::config::Config;

#[derive(Debug, Deserialize)]
pub struct Day {
    pub temp: f64,
    pub sky: String,
    pub wind: Option<f64>,
}

/// Today's weather in `city`, converted to the configured units.
pub fn today(config: &Config, city: &str) -> Result<Day, ureq::Error> {
    let url = format!("{}/today", config.endpoint);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(config.timeout)))
        .build()
        .new_agent();
    let mut day: Day = agent.get(&url).query("city", city).call()?.body_mut().read_json()?;
    day.temp = config.convert(day.temp);
    Ok(day)
}
T
cat > src/cache.rs <<'T'
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Keeps answers for a while so repeated runs skip the network.
pub struct Cache<T> {
    ttl: Duration,
    entries: HashMap<String, (Instant, T)>,
}

impl<T: Clone> Cache<T> {
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, entries: HashMap::new() }
    }

    pub fn get(&self, key: &str) -> Option<T> {
        let (at, value) = self.entries.get(key)?;
        (at.elapsed() < self.ttl).then(|| value.clone())
    }

    pub fn put(&mut self, key: &str, value: T) {
        self.entries.insert(key.to_owned(), (Instant::now(), value));
    }
}
T
sed -i '' 's/on: \[push\]/on: [push, pull_request]/' .github/workflows/ci.yml
cat >> .github/workflows/ci.yml <<'T'
      - run: cargo clippy -- -D warnings
T
git add Cargo.toml
