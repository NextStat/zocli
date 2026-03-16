class Zocli < Formula
  desc "Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents"
  homepage "https://github.com/NextStat/zocli"
  version "0.2.2"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.2/zocli-aarch64-apple-darwin.tar.gz"
      sha256 "10d572d87fb5877a48e5bd6926865394609556dceac0c495969c9169b0662497"
    else
      url "https://github.com/NextStat/zocli/releases/download/v0.2.2/zocli-x86_64-apple-darwin.tar.gz"
      sha256 "a26c0594c6712a77c90c366c04f93ae71ea0092736be4d62e10173f0f32f6a9e"
    end

  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.2/zocli-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "01a963363e582695e69e31a27d55b5a345b95558d3c23ddd9083b70cf1f91900"
    elsif Hardware::CPU.intel?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.2/zocli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "4fad094a67bdceb344840ba3d7aafdd736dcaceec6e4ad4d9e2db5f187cb916a"
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
