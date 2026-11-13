# Updated by scripts/release-macos.sh on each notarized build.
cask "skill-construct" do
  version "0.3.0"
  sha256 "b7efa544cd781c6c387a1fa7f2c6e6eac213f0347cdea484f5fdfad9c5711fe1"

  url "https://github.com/justin-r-wright/skillconstruct/releases/download/v#{version}/Skill-Construct-#{version}-macos.zip"
  name "Skill Construct"
  desc "Manage agent skills across Claude Code, Codex, Cursor and opencode"
  homepage "https://github.com/justin-r-wright/skillconstruct"

  auto_updates true
  depends_on :macos

  app "Skill Construct.app"

  zap trash: "~/.config/skillconstruct"
end
