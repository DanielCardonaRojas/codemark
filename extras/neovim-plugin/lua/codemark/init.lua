local M = {}

-- Try to find codemark binary
local function find_binary()
  -- Check PATH first
  if vim.fn.executable("codemark") == 1 then
    return "codemark"
  end
  -- Check cargo bin
  local cargo_bin = vim.fn.expand("~/.cargo/bin/codemark")
  if vim.fn.executable(cargo_bin) == 1 then
    return cargo_bin
  end
  return "codemark"
end

local config = {
  binary = find_binary(),
  signs = true,
  sign_text = "▎",
  sign_hl = "CodemarkSign",
}

function M.setup(opts)
  config = vim.tbl_deep_extend("force", config, opts or {})

  -- Define highlight and sign
  vim.api.nvim_set_hl(0, "CodemarkSign", { fg = "#61afef", default = true })
  vim.fn.sign_define("codemark_bookmark", {
    text = config.sign_text,
    texthl = config.sign_hl,
  })

  -- Keymaps
  vim.keymap.set("v", "<leader>ba", ":<C-u>CodemarkAdd<CR>", { desc = "Codemark: Add bookmark" })
  vim.keymap.set("v", "<leader>bd", ":<C-u>CodemarkDryRun<CR>", { desc = "Codemark: Dry run" })
  vim.keymap.set("n", "<leader>bb", "<cmd>CodemarkBrowse<CR>", { desc = "Codemark: Browse (Telescope)" })
  vim.keymap.set("n", "<leader>bl", "<cmd>CodemarkList<CR>", { desc = "Codemark: List" })
  vim.keymap.set("n", "<leader>bs", "<cmd>CodemarkStatus<CR>", { desc = "Codemark: Status" })

  -- Auto-place signs on BufEnter
  if config.signs then
    vim.api.nvim_create_autocmd("BufEnter", {
      group = vim.api.nvim_create_augroup("codemark_signs", { clear = true }),
      callback = function()
        M.refresh_signs()
      end,
    })
  end
end

-- Run codemark and return parsed JSON, or nil on error
local function run_json(args)
  if vim.fn.executable(config.binary) ~= 1 then
    return nil, "codemark binary not found: " .. config.binary .. " (install with: cargo install --path /path/to/codemark)"
  end

  local cmd = { config.binary, "--json" }
  for _, a in ipairs(args) do
    table.insert(cmd, a)
  end

  local ok_sys, result = pcall(function()
    return vim.system(cmd, { text = true }):wait()
  end)
  if not ok_sys then
    return nil, "failed to run codemark: " .. tostring(result)
  end
  if result.code ~= 0 then
    return nil, result.stderr or "codemark failed"
  end

  local ok, json = pcall(vim.json.decode, result.stdout)
  if not ok or not json or not json.success then
    return nil, "failed to parse codemark output"
  end
  return json.data
end

-- Run codemark and return raw stdout
local function run_raw(args)
  local cmd = { config.binary }
  for _, a in ipairs(args) do
    table.insert(cmd, a)
  end
  local result = vim.system(cmd, { text = true }):wait()
  return result.stdout or "", result.code
end

-- Get the visual selection line range
local function get_visual_range()
  local start_line = vim.fn.line("'<")
  local end_line = vim.fn.line("'>")
  return start_line, end_line
end

-- Get the relative file path for codemark
local function get_file_path()
  return vim.fn.expand("%:.")
end

--- Dry run: show what would be bookmarked for the visual selection
function M.dry_run_visual()
  local start_line, end_line = get_visual_range()
  local file = get_file_path()
  local range = start_line == end_line and tostring(start_line) or (start_line .. ":" .. end_line)

  local data, err = run_json({ "add", "--file", file, "--range", range, "--dry-run" })
  if not data then
    vim.notify("codemark: " .. (err or "dry run failed"), vim.log.levels.ERROR)
    return
  end

  local lines = {
    "Codemark dry run:",
    "",
    "  Node type:  " .. (data.node_type or "?"),
    "  Name:       " .. (data.name or "(unnamed)"),
    "  File:       " .. (data.file or file),
    "  Lines:      " .. (data.lines or "?"),
    "  Unique:     " .. (data.unique and "yes" or "no") .. " (" .. (data.match_count or "?") .. " matches)",
    "",
    "  Query:",
  }
  for qline in (data.query or ""):gmatch("[^\n]+") do
    table.insert(lines, "    " .. qline)
  end

  vim.notify(table.concat(lines, "\n"), vim.log.levels.INFO)
end

--- Add bookmark from visual selection, prompting for note and tags
function M.add_visual()
  local start_line, end_line = get_visual_range()
  local file = get_file_path()
  local range = start_line == end_line and tostring(start_line) or (start_line .. ":" .. end_line)

  -- First do a dry run to show what will be bookmarked
  local preview, err = run_json({ "add", "--file", file, "--range", range, "--dry-run" })
  if not preview then
    vim.notify("codemark: " .. (err or "failed"), vim.log.levels.ERROR)
    return
  end

  local target = preview.name or preview.node_type or "node"
  local prompt_text = string.format("Bookmark %s (%s)? Note: ", target, preview.lines or "?")

  vim.ui.input({ prompt = prompt_text }, function(note)
    if note == nil then
      return -- cancelled
    end

    vim.ui.input({ prompt = "Tags (comma-separated, or empty): " }, function(tags_input)
      local args = { "add", "--file", file, "--range", range, "--created-by", "user" }

      if note and note ~= "" then
        table.insert(args, "--note")
        table.insert(args, note)
      end

      if tags_input and tags_input ~= "" then
        for tag in tags_input:gmatch("[^,]+") do
          tag = vim.trim(tag)
          if tag ~= "" then
            table.insert(args, "--tag")
            table.insert(args, tag)
          end
        end
      end

      local data, add_err = run_json(args)
      if not data then
        vim.notify("codemark: " .. (add_err or "add failed"), vim.log.levels.ERROR)
        return
      end

      local id = data.id or "?"
      vim.notify(string.format("Bookmark created: %s → %s", id:sub(1, 8), target), vim.log.levels.INFO)
      M.refresh_signs()
    end)
  end)
end

--- List bookmarks for the current file
function M.list()
  local file = get_file_path()
  local data, err = run_json({ "list", "--file", file })
  if not data then
    vim.notify("codemark: " .. (err or "no bookmarks"), vim.log.levels.INFO)
    return
  end

  if #data == 0 then
    vim.notify("No bookmarks for " .. file, vim.log.levels.INFO)
    return
  end

  local lines = { "Bookmarks for " .. file .. ":" }
  for _, bm in ipairs(data) do
    local id = (bm.id or ""):sub(1, 8)
    local note = bm.notes or ""
    local tags = ""
    if bm.tags and #bm.tags > 0 then
      tags = " [" .. table.concat(bm.tags, ", ") .. "]"
    end
    table.insert(lines, string.format("  %s  %s%s  %s", id, bm.status or "?", tags, note))
  end
  vim.notify(table.concat(lines, "\n"), vim.log.levels.INFO)
end

--- Show health status
function M.status()
  local data, err = run_json({ "status" })
  if not data then
    vim.notify("codemark: " .. (err or "status failed"), vim.log.levels.ERROR)
    return
  end
  vim.notify(
    string.format(
      "Codemark: %s active | %s drifted | %s stale | %s archived",
      data.active or 0,
      data.drifted or 0,
      data.stale or 0,
      data.archived or 0
    ),
    vim.log.levels.INFO
  )
end

--- Preview the bookmark under or nearest to the cursor
function M.preview_current()
  local file = get_file_path()
  local line = vim.fn.line(".")

  local data = run_json({ "list", "--file", file })
  if not data or #data == 0 then
    vim.notify("No bookmarks in this file", vim.log.levels.INFO)
    return
  end

  -- Use the first bookmark (could improve to find nearest)
  local bm = data[1]
  local stdout = run_raw({ "preview", bm.id:sub(1, 8), "--no-color" })

  -- Show in a scratch buffer
  local buf = vim.api.nvim_create_buf(false, true)
  local lines = {}
  for l in stdout:gmatch("[^\n]+") do
    table.insert(lines, l)
  end
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.api.nvim_buf_set_option(buf, "modifiable", false)
  vim.api.nvim_buf_set_option(buf, "bufhidden", "wipe")

  -- Open in a split
  vim.cmd("belowright split")
  vim.api.nvim_win_set_buf(0, buf)
  vim.api.nvim_win_set_height(0, math.min(#lines + 1, 25))
end

--- Place signs for bookmarks in the current buffer
function M.refresh_signs()
  local bufnr = vim.api.nvim_get_current_buf()
  local file = get_file_path()

  -- Clear existing codemark signs
  vim.fn.sign_unplace("codemark", { buffer = bufnr })

  -- Skip non-file buffers
  if file == "" or vim.bo[bufnr].buftype ~= "" then
    return
  end

  -- Get bookmarks for this file (async to avoid blocking)
  vim.system(
    { config.binary, "--json", "resolve", "--file", file },
    { text = true },
    function(result)
      if result.code ~= 0 then
        return
      end
      local ok, json = pcall(vim.json.decode, result.stdout)
      if not ok or not json or not json.success or not json.data then
        return
      end

      vim.schedule(function()
        for _, item in ipairs(json.data) do
          local line = item.line
          if line and line > 0 then
            pcall(vim.fn.sign_place, 0, "codemark", "codemark_bookmark", bufnr, { lnum = line })
          end
        end
      end)
    end
  )
end

--- Telescope integration: browse all bookmarks
function M.browse()
  local has_telescope, _ = pcall(require, "telescope")
  if not has_telescope then
    vim.notify("codemark: telescope.nvim is required for :CodemarkBrowse", vim.log.levels.ERROR)
    return
  end

  local pickers = require("telescope.pickers")
  local finders = require("telescope.finders")
  local conf = require("telescope.config").values
  local actions = require("telescope.actions")
  local action_state = require("telescope.actions.state")
  local previewers = require("telescope.previewers")

  pickers
    .new({}, {
      prompt_title = "Codemark Bookmarks",
      finder = finders.new_async_job({
        command_generator = function()
          return { config.binary, "list", "--format", "line" }
        end,
        entry_maker = function(line)
          local parts = vim.split(line, "\t")
          if #parts < 3 then
            return nil
          end
          local id = parts[1]
          local file_loc = parts[2]
          local status = parts[3]
          local tags = parts[4] or ""
          local note = parts[5] or ""

          -- Parse file:line if present
          local file, lnum = file_loc:match("^(.+):(%d+)$")
          if not file then
            file = file_loc
            lnum = 1
          end

          return {
            value = id,
            display = string.format("%-8s %-40s %s %s", id, file_loc, tags, note),
            ordinal = id .. " " .. file_loc .. " " .. tags .. " " .. note,
            filename = file,
            lnum = tonumber(lnum) or 1,
            id = id,
          }
        end,
      }),
      previewer = previewers.new_termopen_previewer({
        get_command = function(entry)
          return { config.binary, "preview", entry.id, "--no-color" }
        end,
      }),
      sorter = conf.generic_sorter({}),
      attach_mappings = function(prompt_bufnr, map)
        actions.select_default:replace(function()
          local entry = action_state.get_selected_entry()
          actions.close(prompt_bufnr)
          if entry and entry.filename then
            vim.cmd("edit " .. entry.filename)
            vim.api.nvim_win_set_cursor(0, { entry.lnum, 0 })
          end
        end)
        return true
      end,
    })
    :find()
end

return M
