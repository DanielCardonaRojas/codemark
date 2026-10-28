-- Sample Lua source used to regression-test the profile generator.
-- It exercises every construct the committed manifest's profile names:
-- named function declarations, anonymous function definitions, local and
-- global variable declarations, and the control-flow kinds that must be
-- labelled but must NOT become landmarks.

local M = {}

local counter = 0

function M.add(a, b)
  if a > b then
    return a + b
  end
  return b - a
end

function M.process(items)
  local total = 0
  for i, item in ipairs(items) do
    total = total + item
  end
  while total > 100 do
    total = total - 10
  end
  return total
end

local helper = function(x)
  return x * 2
end

M.transform = function(v)
  return helper(v)
end

return M
