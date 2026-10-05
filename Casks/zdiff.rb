# A cask, not a formula: brew checks Xcode before installing an unbottled formula.
cask "zdiff" do
  arch arm: "aarch64", intel: "x86_64"
  os macos: "apple-darwin", linux: "unknown-linux-gnu"

  version "0.1.0"
  # .github/workflows/release.yml rewrites the version and checksums on each release.
  sha256 arm:          "70a469b80ca264fe90aaaaa4b6f958bcb81e56850cfd359d830e61247da988e9",
         intel:        "880c1929d438b37e7fb49a71fa99c6095621aae05a32c1fc867da161d0b61965",
         arm64_linux:  "04f3fbf111c69104d1e93f57c57203dbb7f6911c336d7f14cdfd76dff3808cf1",
         x86_64_linux: "f3834841f77ff86db3ae5ad359ac5b307dc66132e6578e7cc09e43bde0fa3025"

  url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-#{arch}-#{os}.tar.gz"
  name "zdiff"
  desc "Terminal git diff viewer with side-by-side view and live refresh"
  homepage "https://github.com/kriptonian1/zdiff"

  binary "zdiff"

  # The binary isn't notarized, so macOS kills a quarantined copy on launch.
  postflight_steps do
    on_macos do
      run "/usr/bin/xattr", args: ["-c", "zdiff"], chdir: "."
    end
  end

  caveats <<~EOS
    macOS and most Linux systems ship an unrelated /usr/bin/zdiff (from gzip).
    If `zdiff --version` doesn't print zdiff #{version}, add this line to the end
    of your shell's startup file (~/.zshrc for zsh, ~/.bashrc for bash), then open
    a new terminal:
      export PATH="#{HOMEBREW_PREFIX}/bin:$PATH"
  EOS
end
