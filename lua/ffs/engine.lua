--- Lua wrapper around the additive `engine_*` exports of ffs-nvim.
---
--- Existing ffs.nvim picker functions (`require('ffs.main')`, etc.) are
--- untouched. This module is purely additive: it gives Neovim Lua callers
--- access to the new symbol index, dispatch, grep, and budgeted read.
---
--- Typical usage:
---   local engine = require('ffs.engine')
---   engine.init(vim.fn.getcwd())
---   for _, hit in ipairs(engine.symbol('FilePicker')) do print(hit.path, hit.line) end
---   local res = engine.read('lua/ffs/main.lua', 5000, 'minimal')

local M = {}

local rust = nil

local function load_rust()
  if rust then return rust end
  rust = require('ffs.rust')
  if type(rust.engine_init) ~= 'function' then
    error(
      'ffs.engine: this build of ffs_nvim was compiled without the `engine` Cargo '
        .. 'feature. Rebuild with `cargo build --release --features engine` '
        .. 'to enable the engine_* exports.'
    )
  end
  return rust
end

--- Build the engine and run the unified scan.
--- @param root string Repository root path (typically `vim.fn.getcwd()`).
--- @param opts table|nil Optional table: `{ total_token_budget = 25000 }`.
--- @return boolean ok
function M.init(root, opts)
  vim.validate({
    root = { root, 'string' },
    opts = { opts, 'table', true },
  })
  return load_rust().engine_init(root, opts)
end

--- Re-run the unified scan, refreshing all caches in place.
--- @return boolean ok
function M.rebuild() return load_rust().engine_rebuild() end

--- Auto-classify and dispatch a free-form query.
--- @param query string
--- @return table result Shape: `{ kind, raw, hits|path|pattern }`.
function M.dispatch(query)
  vim.validate({ query = { query, 'string' } })
  return load_rust().engine_dispatch(query)
end

--- Look up a symbol by exact name (or by prefix when `name` ends in `*`).
--- @param name string
--- @return table[] hits Array of `{ name, path, line, kind }` rows.
function M.symbol(name)
  vim.validate({ name = { name, 'string' } })
  return load_rust().engine_symbol(name)
end

--- Plain-text grep over the workspace.
--- @param pattern string
--- @return table[] hits Array of `{ path, line, text }` rows (capped at 500).
function M.grep(pattern)
  vim.validate({ pattern = { pattern, 'string' } })
  return load_rust().engine_grep(pattern)
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
  return load_rust().engine_read(target, budget, filter)
end

--- Find definitions + single-hop usages of a symbol in one shot.
--- Shells out to the ffs CLI; returns the raw JSON payload as a string.
--- @param name string Symbol name.
--- @param limit integer|nil Maximum usages (default 100).
--- @param offset integer|nil Pagination offset for usages (default 0).
--- @return string json
function M.refs(name, limit, offset)
  vim.validate({
    name = { name, 'string' },
    limit = { limit, 'number', true },
    offset = { offset, 'number', true },
  })
  return load_rust().engine_refs(name, limit, offset)
end

--- Drill-down envelope per definition (def + body + callees + callers).
--- Returns the raw CLI JSON payload as a string.
--- @param name string Symbol name.
--- @param opts table|nil `{ limit, offset, callees_top, callers_top, budget }`.
--- @return string json
function M.flow(name, opts)
  vim.validate({
    name = { name, 'string' },
    opts = { opts, 'table', true },
  })
  return load_rust().engine_flow(name, opts)
end

--- Rank workspace files by how much they'd be affected if `name` changed.
--- Score = direct*3 + imports*2 + transitive*1. Returns the raw CLI JSON.
--- @param name string Symbol name.
--- @param opts table|nil `{ limit, offset, hops, hub_guard }`.
--- @return string json
function M.impact(name, opts)
  vim.validate({
    name = { name, 'string' },
    opts = { opts, 'table', true },
  })
  return load_rust().engine_impact(name, opts)
end

return M
