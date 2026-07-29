# Homebrew formula for aishe.
#
# This is a template that the release process fills in: set `version` and the
# per-target `sha256` values to match the GitHub release artifacts (the
# `aishe-<target>.tar.gz.sha256` files attached by .github/workflows/release.yml).
# Then drop this file into a tap (e.g. homebrew-tap/Formula/aishe.rb).
class Aishe < Formula
  desc "Natural-language-aware shell: zsh for commands, an LLM for everything else"
  homepage "https://github.com/billiondollarsolo/aishe"
  version "0.2.30"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/billiondollarsolo/aishe/releases/download/v#{version}/aishe-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_aarch64-apple-darwin_SHA256"
    end
    on_intel do
      url "https://github.com/billiondollarsolo/aishe/releases/download/v#{version}/aishe-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_x86_64-apple-darwin_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/billiondollarsolo/aishe/releases/download/v#{version}/aishe-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_aarch64-unknown-linux-gnu_SHA256"
    end
    on_intel do
      url "https://github.com/billiondollarsolo/aishe/releases/download/v#{version}/aishe-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_x86_64-unknown-linux-gnu_SHA256"
    end
  end

  def install
    bin.install "aishe"
    # Generate and install shell completions from the binary itself.
    generate_completions_from_executable(bin/"aishe", "completions")
    # Generate and install the man page (aishe emits a roff page itself).
    (buildpath/"aishe.1").write Utils.safe_popen_read(bin/"aishe", "man")
    man1.install "aishe.1"
  end

  test do
    assert_match "aishe", shell_output("#{bin}/aishe --version")
    assert_match "backing shell", shell_output("#{bin}/aishe doctor")
    assert_match ".TH aishe 1", shell_output("#{bin}/aishe man")
  end
end
