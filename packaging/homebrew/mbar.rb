# Draft Homebrew formula for mbar (HEAD-only: there are no tagged releases yet).
#
#   brew tap-new "$USER/local"
#   cp packaging/homebrew/mbar.rb "$(brew --repository "$USER/local")/Formula/"
#   brew install --HEAD "$USER/local/mbar"
#   brew services start mbar
#
# Before submitting to a tap: add a `url`/`sha256` for a tagged release and a stable
# `version`, and run `brew audit --strict --new mbar` and `brew style`.
class Mbar < Formula
  desc "SketchyBar-compatible, GPU-rendered status bar and menu bar replacement"
  homepage "https://github.com/rubenvitt/mbar"
  license "GPL-3.0-only"
  head "https://github.com/rubenvitt/mbar.git", branch: "main"

  depends_on "rust" => :build
  depends_on :macos

  def install
    # Lua 5.4 is vendored (mlua "vendored"), no external dependency needed.
    system "cargo", "install", *std_cargo_args(path: "crates/mbar")
    pkgshare.install "lua/mbar.d.lua"
    pkgshare.install "packaging/dev.rubeen.mbar.plist"
  end

  def caveats
    <<~EOS
      SketchyBar plugins call `sketchybar`. To route them to mbar, create the
      symlink yourself (not done by default, it would conflict with the
      sketchybar formula):
        ln -sf #{opt_bin}/mbar #{HOMEBREW_PREFIX}/bin/sketchybar

      mbar also draws JankyBorders-style window borders and reads
      ~/.config/borders/bordersrc in place. Its `borders …` lines and the
      ones in yabairc / aerospace.toml reach mbar through a `borders`
      symlink. Create it yourself (not done by default, it would conflict
      with felixkratz/formulae/borders; stop and uninstall that first:
      brew services stop borders && brew uninstall borders):
        ln -sf #{opt_bin}/mbar #{HOMEBREW_PREFIX}/bin/borders

      Config: ~/.config/mbar/init.lua or mbarrc (falls back to
      ~/.config/sketchybar/sketchybarrc).

      app_menu items need the Accessibility permission, aliases need Screen
      Recording. Grant both to #{opt_bin}/mbar in System Settings → Privacy &
      Security, then restart: brew services restart mbar

      LuaLS type definitions: #{opt_pkgshare}/mbar.d.lua
    EOS
  end

  service do
    run opt_bin/"mbar"
    environment_variables PATH: std_service_path_env
    keep_alive successful_exit: false
    process_type :interactive
    log_path var/"log/mbar.log"
    error_log_path var/"log/mbar.log"
  end

  test do
    assert_match(/^mbar-v\d+\.\d+\.\d+$/, shell_output("#{bin}/mbar -v"))
    # Invoked as `sketchybar`, mbar reports SketchyBar's version for plugin checks.
    ln_s bin/"mbar", testpath/"sketchybar"
    assert_equal "sketchybar-v2.24.0\n", shell_output("#{testpath}/sketchybar -v")
    # Invoked as `borders`, mbar reports JankyBorders' version.
    ln_s bin/"mbar", testpath/"borders"
    assert_equal "borders-v1.9.0\n", shell_output("#{testpath}/borders -v")
  end
end
