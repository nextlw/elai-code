class Elai < Formula
  desc "CLI de agente de IA"
  homepage "https://github.com/nextlw/elai-code"
  version "1.2.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextlw/elai-code/releases/download/v#{version}/elai-macos-arm64"
      sha256 "b7b0e25551d961b1290549313ada3cf537ff71eea45984c8774d7b579ed381d2"
    end
    on_intel do
      url "https://github.com/nextlw/elai-code/releases/download/v#{version}/elai-macos-x86_64"
      sha256 "d1a8007f9e3a48bb73ff62704f8fa05ff1fe8b64c8aa9c17d41f9b19501fdec6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nextlw/elai-code/releases/download/v#{version}/elai-linux-arm64"
      sha256 "daaea1497b7e5b0cbffcf6663a6f8b72b24301c5607a114dbda729d24f6245d8"
    end
    on_intel do
      url "https://github.com/nextlw/elai-code/releases/download/v#{version}/elai-linux-x86_64"
      sha256 "6546f3417c8c914aa9a0bd6894c36a7e7e04a575ddf6380785424e897133c662"
    end
  end

  def install
    bin.install Dir["elai-*"].first => "elai"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/elai --version")
  end
end
