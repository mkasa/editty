# Homebrew formula for editty (builds from source).
#
# Put this file in a tap repo named `homebrew-tap` (i.e. github.com/<USER>/homebrew-tap)
# at `Formula/editty.rb`, then users install with:
#
#     brew install <USER>/tap/editty
#
# Replace <USER>, the tag in `url`, and `sha256` before publishing. Get the
# checksum with:
#
#     curl -L https://github.com/<USER>/editty/archive/refs/tags/v0.1.0.tar.gz | shasum -a 256
#
class Editty < Formula
  desc "Terminal video editor using the kitty graphics protocol"
  homepage "https://github.com/<USER>/editty"
  url "https://github.com/<USER>/editty/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_SOURCE_TARBALL_SHA256"
  license "MIT" # match the LICENSE file you add to the repo

  depends_on "rust" => :build
  depends_on "ffmpeg" # provides ffmpeg, ffprobe, ffplay at runtime

  def install
    # std_cargo_args == --locked --root #{prefix} --path . (needs Cargo.lock committed)
    system "cargo", "install", *std_cargo_args
  end

  def caveats
    <<~EOS
      editty renders video with the kitty graphics protocol, so run it in a bare
      kitty terminal window (https://sw.kovidgoyal.net/kitty/) — not inside
      tmux/screen, where the protocol does not work.
    EOS
  end

  test do
    assert_match "editty", shell_output("#{bin}/editty --help")
  end
end
