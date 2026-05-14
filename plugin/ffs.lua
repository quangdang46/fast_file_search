if vim.g.ffs_loaded then return end
vim.g.ffs_loaded = true

-- Defer indexing until after UIEnter, so as to not block the UI.
-- This is equivalent to lazy.nvim's VeryLazy, but works with all plugin managers.

local init = vim.schedule_wrap(function()
  if vim.v.exiting ~= vim.NIL then return end
  -- PERF: We query the vim.g.ffs config to avoid eagerly requiring Lua modules
  local lazy_sync = vim.tbl_get(vim.g, 'ffs', 'lazy_sync')
  if lazy_sync == nil or not lazy_sync then require('ffs.core').ensure_initialized() end
end)

if vim.v.vim_did_enter == 1 then
  init()
else
  vim.api.nvim_create_autocmd('UIEnter', {
    group = vim.api.nvim_create_augroup('ffs.main', {}),
    once = true,
    nested = true,
    callback = init,
  })
end

vim.api.nvim_create_user_command('FFSFind', function(opts)
  local ffs = require('ffs')
  if opts.args and opts.args ~= '' then
    -- If argument looks like a directory, use it as base path
    if vim.fn.isdirectory(opts.args) == 1 then
      ffs.find_files_in_dir(opts.args)
    else
      -- Otherwise treat as search query
      ffs.search_and_show(opts.args)
    end
  else
    ffs.find_files()
  end
end, {
  nargs = '?',
  complete = function(arg_lead)
    -- Complete with directories and common search terms
    local dirs = vim.fn.glob(arg_lead .. '*', false, true)
    local results = {}
    for _, dir in ipairs(dirs) do
      if vim.fn.isdirectory(dir) == 1 then table.insert(results, dir) end
    end
    return results
  end,
  desc = 'Find files with ffs (use directory path or search query)',
})

vim.api.nvim_create_user_command('FFSScan', function() require('ffs').scan_files() end, {
  desc = 'Scan files for ffs',
})

vim.api.nvim_create_user_command('FFSRefreshGit', function() require('ffs').refresh_git_status() end, {
  desc = 'Manually refresh git status for all files',
})

vim.api.nvim_create_user_command('FFSClearCache', function(opts) require('ffs').clear_cache(opts.args) end, {
  nargs = '?',
  complete = function() return { 'all', 'frecency', 'files' } end,
  desc = 'Clear ffs caches (all|frecency|files)',
})

vim.api.nvim_create_user_command('FFSHealth', function() vim.cmd('checkhealth ffs') end, {
  desc = 'Check ffs health',
})

vim.api.nvim_create_user_command('FFSDebug', function(opts)
  local config = require('ffs.conf').get()
  if opts.args == 'toggle' or opts.args == '' then
    config.debug.show_scores = not config.debug.show_scores
    config.debug.enabled = config.debug.show_scores
    local status = config.debug.show_scores and 'enabled' or 'disabled'
    vim.notify('ffs debug scores ' .. status, vim.log.levels.INFO)
  elseif opts.args == 'on' then
    config.debug.show_scores = true
    config.debug.enabled = true
    vim.notify('ffs debug scores enabled', vim.log.levels.INFO)
  elseif opts.args == 'off' then
    config.debug.show_scores = false
    config.debug.enabled = false
    vim.notify('ffs debug scores disabled', vim.log.levels.INFO)
  else
    vim.notify('Usage: :FFSDebug [on|off|toggle]', vim.log.levels.ERROR)
  end
end, {
  nargs = '?',
  complete = function() return { 'on', 'off', 'toggle' } end,
  desc = 'Toggle ffs debug scores display',
})

vim.api.nvim_create_user_command('FFSOpenLog', function()
  local ffs = require('ffs')
  local config = require('ffs.conf').get()
  if ffs.log_file_path then
    vim.cmd('tabnew ' .. vim.fn.fnameescape(ffs.log_file_path))
  elseif config and config.logging and config.logging.log_file then
    -- Fallback to the configured log file path even if tracing wasn't initialized
    vim.cmd('tabnew ' .. vim.fn.fnameescape(config.logging.log_file))
  else
    vim.notify('Log file path not available', vim.log.levels.ERROR)
  end
end, {
  desc = 'Open ffs log file in new tab',
})
