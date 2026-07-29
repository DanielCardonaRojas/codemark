local M = {}

function M.greet(name)
  print("hello " .. name)
end

local function add(a, b)
  return a + b
end

M.result = add(1, 2)
return M
