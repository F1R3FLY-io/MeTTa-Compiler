class Mettatron < Formula
  desc "MeTTa language evaluator with lazy evaluation and pattern matching"
  homepage "https://github.com/F1R3FLY-io/MeTTa-Compiler"
  url "https://github.com/F1R3FLY-io/MeTTa-Compiler/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_ACTUAL_SHA256"
  license "Apache-2.0"
  head "https://github.com/F1R3FLY-io/MeTTa-Compiler.git", branch: "main"

  depends_on "rust" => :build
  depends_on "protobuf" => :build

  resource "f1r3node" do
    url "https://github.com/F1R3FLY-io/f1r3node.git",
        using: :git,
        branch: "dylon/mettatron"
  end

  resource "MORK" do
    url "https://github.com/trueagi-io/MORK.git",
        using: :git,
        branch: "main"
  end

  resource "PathMap" do
    url "https://github.com/Adam-Vandervorst/PathMap.git",
        using: :git,
        branch: "master"
  end

  def install
    # Clone dependencies
    (buildpath/"../f1r3node").install resource("f1r3node")
    (buildpath/"../MORK").install resource("MORK")
    (buildpath/"../PathMap").install resource("PathMap")

    # Build
    system "cargo", "install", "--locked", "--root", prefix, "--path", "."

    # Install additional files
    doc.install "README.md"
    doc.install "examples"
  end

  test do
    # Test that the binary runs
    assert_match "MeTTaTron", shell_output("#{bin}/mettatron --help", 2)
  end
end
