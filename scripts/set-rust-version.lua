local version = arg[1]

if not version or not version:match('^%d+%.%d+%.%d+') then
  io.stderr:write('usage: lua scripts/set-rust-version.lua <semver>\n')
  os.exit(1)
end

local files = {
  'Cargo.toml',
  'crates/ffs-c/Cargo.toml',
  'crates/ffs-core/Cargo.toml',
  'crates/ffs-mcp/Cargo.toml',
  'crates/ffs-nvim/Cargo.toml',
  'crates/ffs-query-parser/Cargo.toml',
  'crates/ffs-grep/Cargo.toml',
  'crates/ffs-symbol/Cargo.toml',
  'crates/ffs-budget/Cargo.toml',
  'crates/ffs-engine/Cargo.toml',
  'crates/ffs-cli/Cargo.toml',
}

local function read_file(path)
  local f = assert(io.open(path, 'r'))
  local text = f:read('*a')
  f:close()
  return text
end

local function write_file(path, text)
  local f = assert(io.open(path, 'w'))
  f:write(text)
  f:close()
end

local function update_package_versions(text)
  local out = {}
  local section = nil

  for line in (text .. '\n'):gmatch('(.-)\n') do
    local header = line:match('^%s*%[([^%]]+)%]')
    if header then section = header end

    if section == 'package' then
      line = line:gsub('^(%s*version%s*=%s*")[^"]+(")', '%1' .. version .. '%2')
    end

    out[#out + 1] = line
  end

  return table.concat(out, '\n') .. '\n'
end

local function update_path_dependency_versions(text)
  text = text:gsub('({[^}\n]-path%s*=%s*"[^"]+"[^}\n]-version%s*=%s*")[^"]+(")', '%1' .. version .. '%2')
  return text:gsub('({[^}\n]-version%s*=%s*")[^"]+("[^}\n]-path%s*=%s*"[^"]+"[^}\n]-})', '%1' .. version .. '%2')
end

for _, path in ipairs(files) do
  local text = read_file(path)
  text = update_package_versions(text)
  text = update_path_dependency_versions(text)
  write_file(path, text)
end
