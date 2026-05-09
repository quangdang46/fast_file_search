--- Lua wrapper around the additive `scry_*` exports of fff-nvim.
---
--- Existing fff.nvim picker functions (`require('fff.main')`, etc.) are
--- untouched. This module is purely additive: it gives Neovim Lua callers
--- access to the new symbol index, dispatch, grep, and budgeted read.
---
--- Typical usage:
---   local scry = require('fff.scry')
---   scry.init(vim.fn.getcwd())
---   for _, hit in ipairs(scry.symbol('FilePicker')) do print(hit.path, hit.line) end
---   local res = scry.read('lua/fff/main.lua', 5000, 'minimal')

local M = {}

local rust = nil

local function load_rust()
  if rust then return rust end
  rust = require('fff.rust')
  return rust
end

--- Build the scry engine and run the unified scan.
--- @param root string Repository root path (typically `vim.fn.getcwd()`).
--- @param opts table|nil Optional table: `{ total_token_budget = 25000 }`.
--- @return boolean ok
function M.init(root, opts)
  vim.validate({
    root = { root, 'string' },
    opts = { opts, 'table', true },
  })
  return load_rust().scry_init(root, opts)
end

--- Re-run the unified scan, refreshing all caches in place.
--- @return boolean ok
function M.rebuild()
  return load_rust().scry_rebuild()
end

--- Auto-classify and dispatch a free-form query.
--- @param query string
--- @return table result Shape: `{ kind, raw, hits|path|pattern }`.
function M.dispatch(query)
  vim.validate({ query = { query, 'string' } })
  return load_rust().scry_dispatch(query)
end

--- Look up a symbol by exact name (or by prefix when `name` ends in `*`).
--- @param name string
--- @return table[] hits Array of `{ name, path, line, kind }` rows.
function M.symbol(name)
  vim.validate({ name = { name, 'string' } })
  return load_rust().scry_symbol(name)
end

--- Plain-text grep over the workspace.
--- @param pattern string
--- @return table[] hits Array of `{ path, line, text }` rows (capped at 500).
function M.grep(pattern)
  vim.validate({ pattern = { pattern, 'string' } })
  return load_rust().scry_grep(pattern)
end

--- Read a file with token-budget aware truncation.
--- Budget math: `tokens × ~85% body × 4 bytes/token = effective byte cap`.
--- @param target string `path` or `path:line`.
--- @param budget integer|nil Token budget (default 25000).
--- @param filter string|nil `none` | `minimal` (default) | `aggressive`.
--- @return table result `{ path, body }`.
function M.read(target, budget, filter)
  vim.validate({
    target = { target, 'string' },
    budget = { budget, 'number', true },
    filter = { filter, 'string', true },
  })
  return load_rust().scry_read(target, budget, filter)
end

return M
