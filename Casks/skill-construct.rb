# Updated by scripts/release-macos.sh on each notarized build.
cask "skill-construct" do
  version "0.4.0"
  sha256 "eff062722ec57e10519c40d11f365e339e8302fd677aec3a63853e28ab1be0da"

  url "https://github.com/justin-r-wright/skillconstruct/releases/download/v#{version}/Skill-Construct-#{version}-macos.zip"
  name "Skill Construct"
  desc "Manage agent skills across Claude Code, Codex, Cursor and opencode"
  homepage "https://github.com/justin-r-wright/skillconstruct"

  auto_updates true
  depends_on :macos

  app "Skill Construct.app"

  zap trash: "~/.config/skillconstruct"
end
