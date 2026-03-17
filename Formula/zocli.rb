class Zocli < Formula
  desc "Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents"
  homepage "https://github.com/NextStat/zocli"
  version "0.2.4"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.4/zocli-aarch64-apple-darwin.tar.gz"
      sha256 "1b8159967714abb9939489ec8a07d91515e6e8abfe3fb2561d316e809a0793df"
    else
      url "https://github.com/NextStat/zocli/releases/download/v0.2.4/zocli-x86_64-apple-darwin.tar.gz"
      sha256 "ccf0a7f659219f2833856a1dcecc63339f5bfc69ff6f154aaf2471f70152f04b"
    end

  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.4/zocli-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "a287082c80ca8d9dc3be798445ff15df783e23c3ea09c79c5fd9b84c6e9c7843"
    elsif Hardware::CPU.intel?
      url "https://github.com/NextStat/zocli/releases/download/v0.2.4/zocli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "4d829d5a99c906ac5eb1446782b003285908f1f3fbf4ba99d76e6cf270d1e2ee"
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
