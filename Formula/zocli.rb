class Zocli < Formula
  desc "Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents"
  homepage "https://github.com/NextStat/zocli"
  version "0.2.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.0/zocli-aarch64-apple-darwin.tar.gz"
      sha256 "e47c53019b8e24688beb67b412bcf04d72154fd58f8a3e4b186e53a00ef54f1c"
    else
      url "https://github.com/NextStat/zocli/releases/download/v0.2.0/zocli-x86_64-apple-darwin.tar.gz"
      sha256 "c56360141a24a3f7998d7bf5a5ea389507f0270b0805e081195fe202d182c3ef"
    end

  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.0/zocli-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "fd17b790b425036edd25a0d348597001e3a40c13aadda03d4816e9caed8d9d13"
    elsif Hardware::CPU.intel?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.0/zocli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "a2b6590e3e5b0e91beb4e4214c85e9be76e23f04f55b28fb67ab8952f1a09681"
    else
      odie "zocli Homebrew packages are not published for this Linux CPU."
    end
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
