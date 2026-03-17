class Zocli < Formula
  desc "Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents"
  homepage "https://github.com/NextStat/zocli"
  version "0.2.3"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.3/zocli-aarch64-apple-darwin.tar.gz"
      sha256 "ff13b7e154efc9eb6e04ef4d3a635ab244f983f7027f5162599add9ad4954039"
    else
      url "https://github.com/NextStat/zocli/releases/download/v0.2.3/zocli-x86_64-apple-darwin.tar.gz"
      sha256 "549cb5f9b397a62eebc53aaa363c5ef34a319490134ed2051988b75342a75605"
    end

  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.3/zocli-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "6df5f5e484993e36c47ad0b859026ded3923489104cae968fb82c63b15afdf9e"
    elsif Hardware::CPU.intel?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.3/zocli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "b4cc49a966d0ed54f48269419400487dca6af89466812bcdaf6af11114952b91"
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
