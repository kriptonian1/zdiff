class Zdiff < Formula
  desc "Terminal git diff viewer with side-by-side view and live refresh"
  homepage "https://github.com/kriptonian1/zdiff"
  version "0.1.0"
  license "MIT"

  # .github/workflows/release.yml rewrites the version and checksums on each release.
  on_macos do
    on_arm do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-aarch64-apple-darwin.tar.gz"
      sha256 "70a469b80ca264fe90aaaaa4b6f958bcb81e56850cfd359d830e61247da988e9"
    end
    on_intel do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-x86_64-apple-darwin.tar.gz"
      sha256 "880c1929d438b37e7fb49a71fa99c6095621aae05a32c1fc867da161d0b61965"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "04f3fbf111c69104d1e93f57c57203dbb7f6911c336d7f14cdfd76dff3808cf1"
    end
    on_intel do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "f3834841f77ff86db3ae5ad359ac5b307dc66132e6578e7cc09e43bde0fa3025"
    end
  end

  def install
    bin.install "zdiff"
  end

  def caveats
    <<~EOS
      macOS and most Linux systems ship an unrelated /usr/bin/zdiff (from gzip).
      If `zdiff --version` doesn't print zdiff #{version}, put #{HOMEBREW_PREFIX}/bin
      before /usr/bin in your PATH:
        export PATH="#{HOMEBREW_PREFIX}/bin:$PATH"
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/zdiff --version")
    assert_match "not a git worktree", shell_output("#{bin}/zdiff 2>&1", 1)
  end
end
