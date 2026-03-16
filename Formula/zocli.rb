class Zocli < Formula
  desc "Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents"
  homepage "https://github.com/NextStat/zocli"
  version "0.1.19"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.1.19/zocli-aarch64-apple-darwin.tar.gz"
      sha256 "2c393159b174f6becbe88b41f55d3d519ddc094cf49012651ca3d3c3ad647852"
    else
      url "https://github.com/NextStat/zocli/releases/download/v0.1.19/zocli-x86_64-apple-darwin.tar.gz"
      sha256 "50f3ee8a1ba40ed3c4c6fd726c0e16275d1535830e6d955015f4fd938356d0ca"
    end

  end

  on_linux do
    odie "zocli Homebrew packages are not published for Linux yet. Use install.sh or cargo install."
  end

  def install
    bin.install "zocli"
    doc.install "README.md", "LICENSE"
  end

  test do
    output = shell_output("#{bin}/zocli --help")
    assert_match "Zoho Mail, Calendar, and WorkDrive", output
  end
end
