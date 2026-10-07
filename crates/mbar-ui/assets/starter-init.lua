-- mbar starter config. Docs: https://github.com/rubenvitt/mbar/blob/main/docs/LUA.md
local mbar = require("mbar")

mbar.bar({ height = 32, color = 0xe01e1e2e, padding_left = 8, padding_right = 8 })
mbar.default({
  icon = { font = "SF Pro:Semibold:14.0", color = 0xffcdd6f4 },
  label = { font = "SF Pro:Semibold:13.0", color = 0xffcdd6f4 },
  padding_left = 6,
  padding_right = 6,
})

mbar.add("item", "front_app", { position = "left", provider = { "front_app" } })
mbar.add("item", "clock", { position = "right", provider = { "clock", args = "%a %d.%m. %H:%M" } })
mbar.add("item", "battery", { position = "right", provider = { "battery", format = "{percent}%" } })
mbar.add("item", "cpu", { position = "right", provider = { "cpu", format = "CPU {percent}%" } })
