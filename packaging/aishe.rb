# Homebrew formula for aishe.
#
# This is a template that the release process fills in: set `version` and the
# per-target `sha256` values to match the GitHub release artifacts (the
# `aishe-<target>.tar.gz.sha256` files attached by .github/workflows/release.yml).
# Then drop this file into a tap (e.g. homebrew-tap/Formula/aishe.rb).
class Aishe < Formula
  desc "Natural-language-aware shell: zsh for commands, an LLM for everything else"
  homepage "https://github.com/billiondollarsolo/aishe"
  version "0.5.0"
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

  def caveats
    <<~EOS
      Run `aishe setup` to install and checksum-verify Aishe's pinned OpenCode
      runtime in your user data directory. Aishe does not use an arbitrary
      Homebrew OpenCode version.

      macOS workspace policy checks are available, but this release does not
      provide an OS sandbox for yolo actions; each yolo shell warns explicitly.
    EOS
  end

  test do
    assert_match "aishe", shell_output("#{bin}/aishe --version")
    assert_match ".TH aishe 1", shell_output("#{bin}/aishe man")
  end
end
