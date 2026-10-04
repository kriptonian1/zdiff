class Zdiff < Formula
  desc "Terminal git diff viewer with side-by-side view and live refresh"
  homepage "https://github.com/kriptonian1/zdiff"
  version "0.1.0"
  license "MIT"

  # .github/workflows/release.yml rewrites the version and checksums on each release.
  on_macos do
    on_arm do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/kriptonian1/zdiff/releases/download/v#{version}/zdiff-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "zdiff"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/zdiff --version")
    assert_match "not a git worktree", shell_output("#{bin}/zdiff 2>&1", 1)
  end
end
