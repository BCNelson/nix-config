-- Strip tracking from copied URLs with the ClearURLs rules
-- (https://gitlab.com/anti-tracking/ClearURLs/rules, LGPL-3.0).
--
-- spool.nix prepends `local RULES = "<store path of data.min.json>"`.
-- Applies the rules the way the ClearURLs add-on does: a provider counts
-- when its urlPattern matches and none of its exceptions do; a redirection
-- (e.g. google.com/url?q=...) is replaced by its target, which is cleaned
-- again; rawRules are cut out of the whole URL; rules and referralMarketing
-- name query / fragment parameters (whole name, any case) to remove.
-- completeProvider (block the site) and forceRedirection are browser
-- concerns and ignored. Only a copy that is a single URL is touched; the
-- original stays as an earlier version of the item.

-- Affiliate tags (amazon `tag=`) are removed too, like the add-on's default.
local ALLOW_REFERRAL_MARKETING = false
-- A redirection target is itself cleaned; stop after this many hops.
local MAX_REDIRECTS = 4
-- Memory: each compiled regex costs ~18 kB, and all 206 providers together
-- ~9 MB, so providers are compiled when a URL first needs them. These are
-- compiled at load and never evicted (globalRules applies to every URL).
local WARM = { globalRules = true, google = true, youtube = true, amazon = true }
-- At most this many other providers stay compiled (least recently used go).
local MAX_COMPILED = 64

local function compile(pattern)
  local ok, re = pcall(spool.regex, "(?i)" .. pattern)
  if ok then return re end
  return nil
end

-- One regex for a list of parameter-name rules: ^(?:r1|r2|...)$.
local function names_regex(list)
  if #list == 0 then return nil end
  local parts = {}
  for i, r in ipairs(list) do parts[i] = "(?:" .. r .. ")" end
  return compile("^(?:" .. table.concat(parts, "|") .. ")$")
end

local function compile_all(list)
  local out = {}
  for _, p in ipairs(list or {}) do
    local re = compile(p)
    if re then out[#out + 1] = re end
  end
  return out
end

-- A literal every URL matching `pattern` must contain (lowercased), e.g.
-- "amazon" from ^https?:\/\/(?:[a-z0-9-]+\.)*?amazon(?:\.[a-z]{2,}){1,}:
-- the longest run of literal characters outside any group, not followed by
-- a quantifier. nil (always check the provider) for a top-level
-- alternation or nothing better than the scheme.
local function required_literal(pattern)
  local depth, runs, cur, i = 0, {}, "", 1
  local function cut() runs[#runs + 1] = cur; cur = "" end
  while i <= #pattern do
    local c = pattern:sub(i, i)
    local ch
    if c == "\\" then
      local n = pattern:sub(i + 1, i + 1)
      i = i + 2
      if n:match("^[./-]$") then ch = n else cut() end
    elseif c == "(" then depth = depth + 1; cut(); i = i + 1
    elseif c == ")" then depth = depth - 1; cut(); i = i + 1
    elseif c == "|" and depth == 0 then return nil
    elseif depth == 0 and c:match("^[%w-]$") then ch = c; i = i + 1
    else cut(); i = i + 1
    end
    if ch and depth == 0 then
      if pattern:sub(i, i):match("^[?*{]$") then cut() else cur = cur .. ch end
    end
  end
  cut()
  local best
  for _, r in ipairs(runs) do
    if #r >= 3 and not ("https://"):find(r, 1, true) and (not best or #r > #best) then best = r end
  end
  return best and best:lower()
end

-- Provider specs (strings only); compiled forms live in `compiled`.
local specs, compiled, uses, clock = {}, {}, {}, 0
local warm_n, lazy_n = 0, 0

local function build(spec)
  local pattern = compile(spec.urlPattern)
  if not pattern then return false end
  return {
    name = spec.name,
    pattern = pattern,
    exceptions = compile_all(spec.exceptions),
    redirections = compile_all(spec.redirections),
    raw = compile_all(spec.raw),
    params = names_regex(spec.params) or false,
  }
end

-- The compiled provider for `spec` (false: its pattern does not compile).
local function get(spec)
  local p = compiled[spec.name]
  clock = clock + 1
  uses[spec.name] = clock
  if p ~= nil then return p end
  p = build(spec)
  compiled[spec.name] = p
  if not WARM[spec.name] then
    lazy_n = lazy_n + 1
    if lazy_n > MAX_COMPILED then
      local oldest, at
      for name in pairs(compiled) do
        if not WARM[name] and name ~= spec.name and (not at or uses[name] < at) then
          oldest, at = name, uses[name]
        end
      end
      if oldest then compiled[oldest] = nil; lazy_n = lazy_n - 1 end
    end
  end
  return p
end

do
  local f = assert(io.open(RULES, "rb"))
  local data = spool.json.decode(f:read("a"))
  f:close()
  local names = {}
  for name in pairs(data.providers) do names[#names + 1] = name end
  table.sort(names) -- JSON object order is lost in Lua; make it stable
  local always = 0
  for _, name in ipairs(names) do
    local p = data.providers[name]
    local params = {}
    for _, r in ipairs(p.rules or {}) do params[#params + 1] = r end
    if not ALLOW_REFERRAL_MARKETING then
      for _, r in ipairs(p.referralMarketing or {}) do params[#params + 1] = r end
    end
    local spec = {
      name = name,
      urlPattern = p.urlPattern,
      literal = required_literal(p.urlPattern),
      exceptions = p.exceptions,
      redirections = p.redirections,
      raw = p.rawRules,
      params = params,
    }
    if not spec.literal then always = always + 1 end
    specs[#specs + 1] = spec
    if WARM[name] then get(spec); warm_n = warm_n + 1 end
  end
  spool.log(("strip-tracking: %d providers (%d warm, %d without a domain literal)")
    :format(#specs, warm_n, always))
end

local function percent_decode(s)
  return (s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end))
end

-- Drop `k=v` segments of a query / fragment string whose (decoded) name
-- matches `re`; the segments that stay are kept byte for byte.
local function filter_params(s, re)
  if not s or s == "" or not re then return s, false end
  local kept, changed = {}, false
  for seg in (s .. "&"):gmatch("([^&]*)&") do
    local name = percent_decode((seg:match("^([^=]*)") or ""):gsub("%+", " "))
    if seg ~= "" and re:is_match(name) then
      changed = true
    elseif seg ~= "" then
      kept[#kept + 1] = seg
    end
  end
  if not changed then return s, false end
  return table.concat(kept, "&"), true
end

local function any_match(list, s)
  for _, re in ipairs(list) do
    if re:is_match(s) then return true end
  end
  return false
end

local function clean(url, hops)
  hops = hops or 0
  local lower = url:lower()
  for _, spec in ipairs(specs) do
    -- Cheap prefilter: no literal in the URL, no match (no regex needed).
    local p = (not spec.literal or lower:find(spec.literal, 1, true)) and get(spec)
    if p and p.pattern:is_match(url) and not any_match(p.exceptions, url) then
      if hops < MAX_REDIRECTS then
        for _, re in ipairs(p.redirections) do
          local c = re:captures(url)
          if c and c[1] then
            local target = percent_decode(c[1])
            if target:match("^https?://") then return clean(target, hops + 1) end
          end
        end
      end
      for _, re in ipairs(p.raw) do url = re:replace(url, "") end
      lower = url:lower()
      local u = spool.url.parse(url)
      if not u then return url end
      local q, qc = filter_params(u.query_string, p.params)
      -- Only fragments that look like parameters (#a=b&c=d), not anchors.
      local frag, fc = u.fragment, false
      if frag and frag:find("=", 1, true) then frag, fc = filter_params(frag, p.params) end
      if qc or fc then
        u.query_string = q
        u.fragment = (frag ~= "" and frag) or nil
        url = spool.url.build(u)
        lower = url:lower()
      end
    end
  end
  return url
end

spool.filter(function(item)
  local text = item.text
  if not text then return end
  local url = text:match("^%s*(https?://%S+)%s*$")
  if not url then return end
  local cleaned = clean(url)
  if cleaned == url then return end
  return spool.keep{ text = cleaned }
end)
