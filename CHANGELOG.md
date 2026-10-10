# Changelog

## [0.5.0](https://github.com/rubenvitt/mbar/compare/v0.4.0...v0.5.0) (2026-10-10)


### Features

* AeroSpace integration (native events, provider, Lua API) ([#22](https://github.com/rubenvitt/mbar/issues/22)) ([5ea6405](https://github.com/rubenvitt/mbar/commit/5ea6405ccb8d209bd1caa0a44987f42b4bae1215))

## [0.4.0](https://github.com/rubenvitt/mbar/compare/v0.3.3...v0.4.0) (2026-10-09)


### Features

* window borders (JankyBorders take-over) ([#20](https://github.com/rubenvitt/mbar/issues/20)) ([442e605](https://github.com/rubenvitt/mbar/commit/442e605b287e91c2d5826d0b30d88416b751bd63))

## [0.3.3](https://github.com/rubenvitt/mbar/compare/v0.3.2...v0.3.3) (2026-10-08)


### Bug Fixes

* **macos:** no Dock icon for the bar daemon ([#17](https://github.com/rubenvitt/mbar/issues/17)) ([b360825](https://github.com/rubenvitt/mbar/commit/b360825fe6bb6b93ce38c078a0f7152efcd99ac7))

## [0.3.2](https://github.com/rubenvitt/mbar/compare/v0.3.1...v0.3.2) (2026-10-08)


### Bug Fixes

* **macos:** reopening mbar.app shows the settings window, quitting it keeps the bar ([#15](https://github.com/rubenvitt/mbar/issues/15)) ([f00aa56](https://github.com/rubenvitt/mbar/commit/f00aa56a1c3b5b338bd5c68e99910b30c0332601))

## [0.3.1](https://github.com/rubenvitt/mbar/compare/v0.3.0...v0.3.1) (2026-10-08)


### Bug Fixes

* **ui:** opening mbar.app starts the bar when it is not running ([#13](https://github.com/rubenvitt/mbar/issues/13)) ([5938a59](https://github.com/rubenvitt/mbar/commit/5938a592ec712c0fe16ded22d989335f1be93fff))

## [0.3.0](https://github.com/rubenvitt/mbar/compare/v0.2.0...v0.3.0) (2026-10-08)


### Features

* **macos:** mbar app icon ([#11](https://github.com/rubenvitt/mbar/issues/11)) ([f7a5c5e](https://github.com/rubenvitt/mbar/commit/f7a5c5eb056fab1f4a2ffc1794048f548871b91c))


### Bug Fixes

* **ui:** restart mbar off the UI thread; re-register the login item after the legacy agent is removed ([#10](https://github.com/rubenvitt/mbar/issues/10)) ([8956808](https://github.com/rubenvitt/mbar/commit/89568083cc8ad72ad1eefccd23d6116d3c22dfdc))

## [0.2.0](https://github.com/rubenvitt/mbar/compare/v0.1.0...v0.2.0) (2026-10-08)


### Features

* mbar.app foundation (Linux tasks of the distribution plan) ([#2](https://github.com/rubenvitt/mbar/issues/2)) ([29909d6](https://github.com/rubenvitt/mbar/commit/29909d6bd56d9f57b217b01183feda5d4fcbf235))
* Sparkle updates, login item and first-launch setup in mbar.app ([#7](https://github.com/rubenvitt/mbar/issues/7)) ([923d906](https://github.com/rubenvitt/mbar/commit/923d90602eb3bafe25eef0fdf981dc61d606edcd))


### Bug Fixes

* **macos:** keep bar windows out of the menu-bar frame constraint ([#5](https://github.com/rubenvitt/mbar/issues/5)) ([986581d](https://github.com/rubenvitt/mbar/commit/986581d9694b6b29e46ad1e6e4cb48098071aa51))
* **macos:** refresh battery provider on percent-change notifications ([#3](https://github.com/rubenvitt/mbar/issues/3)) ([ff56a3b](https://github.com/rubenvitt/mbar/commit/ff56a3b5b7e040778f2187fed9f3cb3bdfe45cee))
