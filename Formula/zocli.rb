class Zocli < Formula
  desc "Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents"
  homepage "https://github.com/NextStat/zocli"
  version "0.1.33"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.1.33/zocli-aarch64-apple-darwin.tar.gz"
      sha256 "130385c50f8b681ad563b66532ea0180c3f4afe868867eb38ff2b076df0f53ca"
    else
      url "https://github.com/NextStat/zocli/releases/download/v0.1.33/zocli-x86_64-apple-darwin.tar.gz"
      sha256 "f966dc8eea91d1ba459f039a4ac4bd51135872e34c29a9ccb126d7e326125b3d"
    end

  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.1.33/zocli-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "f1f3d73005e1212b79788dd06627e264a2385703aeb75c67bb2117c1f542b457"
    elsif Hardware::CPU.intel?
      url "https://github.com/NextStat/zocli/releases/download/v0.1.33/zocli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "46fb8106b8e17450619a28c6478437388e4ff233ff83da5e529e01ed3843c63e"
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
