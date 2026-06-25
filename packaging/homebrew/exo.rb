class Exo < Formula
  desc "Container runtime for AI agents"
  homepage "https://github.com/clawpen/exo"
  url "https://github.com/clawpen/exo/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "PLACEHOLDER_SHA256"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/exo")
  end

  test do
    system "#{bin}/exo", "--version"
  end
end
