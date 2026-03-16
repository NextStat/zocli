class Zocli < Formula
  desc "Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents"
  homepage "https://github.com/NextStat/zocli"
  version "0.2.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.1/zocli-aarch64-apple-darwin.tar.gz"
      sha256 "e1f449796ce294dfd9dc32562198eb842974398924b355fb70e6f09b14595456"
    else
      url "https://github.com/NextStat/zocli/releases/download/v0.2.1/zocli-x86_64-apple-darwin.tar.gz"
      sha256 "eedf801f3b2869cb1b38c8c1de58341640c7d8239babd0a58e83fc1e4bc75d8c"
    end

  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.1/zocli-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c2d6a94ca03067012006df9c196fd18f63d48b5450bda4713daa988307ec9f61"
    elsif Hardware::CPU.intel?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.1/zocli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "93dd6e3b20e02ee64fd30e1346e65f806d045394ecf64180657b43d9abfcb095"
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
