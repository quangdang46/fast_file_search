import { useState, useEffect, useRef, useCallback, type ReactElement } from 'react'
import './App.css'

/* ─── Types ─────────────────────────────────────────── */
interface OverviewData {
  files: number; code_lines: number; languages: { lang: string; files: number }[]
}

interface FileData { pattern: string; matches: string[] }
interface SymHit { n: string; k: string; f: string; l: number }
interface SymData { q: string; hits: SymHit[] }
interface CallHit { f: string; l: number; txt: string; target: string; d: number }
interface CallData { q: string; hits: CallHit[] }
interface GrepMatch { f: string; l: number; txt: string }
interface GrepData { q: string; matches: GrepMatch[] }

type Tab = 'files' | 'symbol' | 'callers' | 'grep' | 'about'

/* ─── Fuzzy scoring ─────────────────────────────────── */
function fuzzyScore(query: string, text: string): number {
  const q = query.toLowerCase(); const t = text.toLowerCase()
  if (t.includes(q)) return q.length * 10 + 100
  let qi = 0, score = 0
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) { score += qi > 0 && ti > 0 && t[ti - 1] === q[qi - 1] ? 5 : 1; qi++ }
  }
  return qi === q.length ? score + 50 : 0
}

function highlight(text: string, q: string) {
  if (!q.trim()) return text
  const ql = q.toLowerCase(); const tl = text.toLowerCase()
  const parts: ReactElement[] = []; let last = 0
  for (let i = 0; i <= tl.length - ql.length; i++) {
    if (tl.slice(i, i + ql.length) === ql) {
      if (i > last) parts.push(<span key={last}>{text.slice(last, i)}</span>)
      parts.push(<mark key={i}>{text.slice(i, i + ql.length)}</mark>)
      last = i + ql.length
    }
  }
  if (last < text.length) parts.push(<span key={last}>{text.slice(last)}</span>)
  return parts.length ? parts : text
}

function ext(path: string) { const i = path.lastIndexOf('.'); return i > 0 ? path.slice(i + 1) : '' }
function langBadge(path: string) {
  const m: Record<string, string> = { rs: 'Rust', ts: 'TS', tsx: 'TSX', js: 'JS', jsx: 'JSX', py: 'Python', sh: 'Shell', toml: 'TOML', md: 'Markdown', css: 'CSS', c: 'C', h: 'C', json: 'JSON', lua: 'Lua' }
  return m[ext(path)] || ''
}

/* ─── ────────────────────────────────────────────────── */
function TabBtn({ tab, current, label, on }: { tab: Tab; current: Tab; label: string; on: (t: Tab) => void }) {
  return <button className={`tab${tab === current ? ' active' : ''}`} onClick={() => on(tab)}>{label}</button>
}

export default function App() {
  const [tab, setTab] = useState<Tab>('files')
  const [overview, setOverview] = useState<OverviewData | null>(null)
  const [files, setFiles] = useState<string[]>([])
  const [q, setQ] = useState('')
  const [results, setResults] = useState<Array<{ path: string; score: number }>>([])

  // Symbols
  const [symQ, setSymQ] = useState('')
  const [symResults, setSymResults] = useState<SymHit[]>([])

  // Callers
  const [callerQ, setCallerQ] = useState('')
  const [callerResults, setCallerResults] = useState<CallHit[]>([])

  // Grep
  const [grepQ, setGrepQ] = useState('')
  const [grepResults, setGrepResults] = useState<GrepMatch[]>([])

  const inputRef = useRef<HTMLInputElement>(null)

  /* Load static data */
  useEffect(() => {
    Promise.all([
      fetch('/data/files.json').then(r => r.json() as Promise<FileData>),
      fetch('/data/overview.json').then(r => r.json() as Promise<OverviewData>),
    ]).then(([fd, ov]) => {
      setFiles(fd.matches)
      setOverview(ov)
      setResults(fd.matches.map(p => ({ path: p, score: 0 })))
    })
  }, [])

  /* Tab switch → re-fetch if needed */
  const loadSymbols = useCallback(async (query: string) => {
    try {
      const r = await fetch(`/data/sym_${query}.json`)
      if (!r.ok) return
      const d: SymData = await r.json()
      setSymResults(d.hits || [])
    } catch { /* preloaded, ok */ }
  }, [])

  const loadCallers = useCallback(async (query: string) => {
    try {
      const r = await fetch(`/data/callers_${query}.json`)
      if (!r.ok) return
      const d: CallData = await r.json()
      setCallerResults(d.hits || [])
    } catch { /* ok */ }
  }, [])

  const loadGrep = useCallback(async (query: string) => {
    const sfx = query.replace(/ /g, '_')
    try {
      const r = await fetch(`/data/grep_${sfx}.json`)
      if (!r.ok) return
      const d: GrepData = await r.json()
      setGrepResults(d.matches || [])
    } catch { /* ok */ }
  }, [])

  /* Keyboard shortcuts */
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { setQ(''); inputRef.current?.focus() }
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [])

  /* ─── File search handler ─── */
  const doSearch = useCallback((query: string) => {
    setQ(query)
    const trimmed = query.trim().toLowerCase()
    if (!trimmed) { setResults(files.map(p => ({ path: p, score: 0 }))); return }
    setResults(files.map(p => ({ path: p, score: fuzzyScore(trimmed, p) }))
      .filter(r => r.score > 0).sort((a, b) => b.score - a.score))
  }, [files])

  /* ─── Quick presets ─── */
  const presets = (tab === 'files')
    ? ['fuzee', 'calers', 'disptch', 'piker', 'grep']
    : tab === 'symbol'
    ? ['run', 'new', 'default', 'find', 'search', 'parse']
    : tab === 'callers'
    ? ['run', 'new', 'find', 'search', 'fuzzy_search', 'dispatch']
    : ['fn_', 'TODO', 'unsafe', 'RwLock', 'fuzzy_file_search']

  /* ─── Render ─── */
  return (
    <>
      <header className="header">
        <h1>🔎 FFS Demo</h1>
        <span className="badge">v0.1.11</span>
        {overview && <span className="stat" style={{ marginLeft: 'auto' }}><span className="num">{overview.files}</span> files · <span className="num">{(overview.code_lines / 1000).toFixed(1)}k</span> lines</span>}
      </header>

      {/* Tabs */}
      <nav className="tabs">
        <TabBtn tab="files" current={tab} label="📁 Find Files" on={setTab} />
        <TabBtn tab="symbol" current={tab} label="◎ Symbols" on={setTab} />
        <TabBtn tab="callers" current={tab} label="↗ Callers" on={setTab} />
        <TabBtn tab="grep" current={tab} label="🔍 Grep" on={setTab} />
        <TabBtn tab="about" current={tab} label="ℹ About" on={setTab} />
      </nav>

      {/* Search bar */}
      {tab !== 'about' && (
        <div className="search-wrap">
          <div className="search-bar">
            <svg className="icon" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="6.5" cy="6.5" r="4.5" /><line x1="10" y1="10" x2="14" y2="14" />
            </svg>
            <input
              ref={inputRef}
              type="text"
              placeholder={
                tab === 'files' ? "Search files — try typos like 'fuzee' or 'calers'…" :
                tab === 'symbol' ? 'Search symbols (pre-indexed: run, new, default, find…)' :
                tab === 'callers' ? "Find callers (pre-indexed: run, new, find, fuzzy_search…)" :
                "Grep content (pre-indexed: fn_, TODO, unsafe, RwLock…)"
              }
              value={q}
              onChange={e => {
                setQ(e.target.value)
                if (tab === 'files') doSearch(e.target.value)
                if (tab === 'symbol') { setSymQ(e.target.value); loadSymbols(e.target.value) }
                if (tab === 'callers') { setCallerQ(e.target.value); loadCallers(e.target.value) }
                if (tab === 'grep') { setGrepQ(e.target.value); loadGrep(e.target.value) }
              }}
              autoFocus
              spellCheck={false}
            />
          </div>
          <div className="search-meta">
            <div className="presets">
              {presets.map(p => <button key={p} className="preset-btn" onClick={() => {
                setQ(p)
                if (tab === 'files') doSearch(p)
                if (tab === 'symbol') { setSymQ(p); loadSymbols(p) }
                if (tab === 'callers') { setCallerQ(p); loadCallers(p) }
                if (tab === 'grep') { setGrepQ(p); loadGrep(p) }
              }}>{p}</button>)}
            </div>
            <span className="hint"><kbd>Esc</kbd> clear</span>
          </div>
        </div>
      )}

      {/* ─── Content ─── */}
      <div className="results">
        {tab === 'files' && <FileResults files={results} query={q} />}
        {tab === 'symbol' && <SymbolResults hits={symResults} query={symQ} overview={overview} />}
        {tab === 'callers' && <CallerResults hits={callerResults} query={callerQ} />}
        {tab === 'grep' && <GrepResults matches={grepResults} query={grepQ} />}
        {tab === 'about' && <AboutPanel overview={overview} />}
      </div>
    </>
  )
}

/* ─── File Results ─── */
function FileResults({ files, query }: { files: Array<{ path: string; score: number }>; query: string }) {
  if (!query.trim()) {
    return (
      <div className="empty-state">
        <div className="big-icon">📁</div>
        <h3>FFS — Fuzzy File Search</h3>
        <p>Type a filename above to search the FFS codebase. FFS is typo-tolerant: try <strong>fuzee</strong> for <code>fuzzy_file_search.rs</code> or <strong>calers</strong> for <code>callees.rs</code>.</p>
        <div className="hint"><kbd>Tab</kbd> switch modes · <kbd>↑↓</kbd> navigate</div>
      </div>
    )
  }
  if (files.length === 0) {
    return (
      <div className="empty-state">
        <div className="big-icon">❓</div>
        <h3>No matches</h3>
        <p>Nothing matches <strong>&ldquo;{query}&rdquo;</strong>. FFS uses fuzzy matching — try fewer chars or a different spelling.</p>
      </div>
    )
  }
  return <>
    <div className="result-count">{files.length} result{files.length !== 1 ? 's' : ''} for &ldquo;{query}&rdquo;</div>
    {files.map(r => (
      <div key={r.path} className="file-item" title={r.path}>
        {ext(r.path) && <span className="ext">{ext(r.path)}</span>}
        <span className="path">{highlight(r.path, query)}</span>
        {langBadge(r.path) && <span className="lang">{langBadge(r.path)}</span>}
      </div>
    ))}
  </>
}

/* ─── Symbol Results ─── */
function SymbolResults({ hits, query, overview }: { hits: SymHit[]; query: string; overview: OverviewData | null }) {
  const kinds = [...new Set(hits.map(h => h.k))].sort()
  const byKind = Object.fromEntries(kinds.map(k => [k, hits.filter(h => h.k === k)]))

  if (!query.trim()) {
    return (
      <div className="empty-state">
        <div className="big-icon">◎</div>
        <h3>Symbol Search</h3>
        <p>Search for function definitions, structs, enums, traits, and more. Try <strong>run</strong>, <strong>new</strong>, <strong>default</strong>, or <strong>find</strong>.</p>
        {overview && <p style={{ fontSize: 12, color: 'var(--text-muted)' }}>{overview.files} files · symbol index: tree-sitter (15 languages)</p>}
      </div>
    )
  }
  if (hits.length === 0) return <div className="empty-state"><div className="big-icon">◎</div><h3>No symbols found</h3><p>No results for &ldquo;{query}&rdquo; — try one of the preset buttons above.</p></div>

  return <>
    <div className="result-count">{hits.length} symbol{hits.length !== 1 ? 's' : ''} for &ldquo;{query}&rdquo;</div>
    {kinds.map(kind => (
      <div key={kind}>
        <div className="section-title">{kind.replace('_', ' ')}</div>
        {byKind[kind].slice(0, 15).map((h, i) => (
          <div key={i} className="file-item" title={`${h.f}:${h.l}`}>
            <span className="sym-kind">{h.k[0]}</span>
            <span className="path"><strong>{h.n}</strong> <span style={{ color: 'var(--text-muted)' }}>@{h.f}:{h.l}</span></span>
          </div>
        ))}
      </div>
    ))}
  </>
}

/* ─── Caller Results ─── */
function CallerResults({ hits, query }: { hits: CallHit[]; query: string }) {
  const byFile: Record<string, CallHit[]> = {}
  for (const h of hits) { if (!byFile[h.f]) byFile[h.f] = []; byFile[h.f].push(h) }

  if (!query.trim()) return (
    <div className="empty-state">
      <div className="big-icon">↗</div>
      <h3>Callers</h3>
      <p>See who calls a symbol. Enter a function name, or try <strong>run</strong>, <strong>new</strong>, <strong>find</strong>, or <strong>fuzzy_search</strong>.</p>
    </div>
  )
  if (hits.length === 0) return <div className="empty-state"><div className="big-icon">↗</div><h3>No callers found</h3><p>No callers for &ldquo;{query}&rdquo; — try one of the preset buttons.</p></div>

  return <>
    <div className="result-count">{hits.length} caller{hits.length !== 1 ? 's' : ''} of &ldquo;{query}&rdquo; ({new Set(hits.map(h => h.f)).size} files)</div>
    {Object.entries(byFile).slice(0, 10).map(([file, calls]) => (
      <div key={file}>
        <div className="section-title">{file}</div>
        {calls.slice(0, 8).map((c, i) => (
          <div key={i} className="file-item" style={{ fontFamily: 'var(--font)' }}>
            <span className="call-line">L{c.l}</span>
            <code className="call-text">&ldquo;{c.txt}&rdquo;</code>
          </div>
        ))}
      </div>
    ))}
  </>
}

/* ─── Grep Results ─── */
function GrepResults({ matches, query }: { matches: GrepMatch[]; query: string }) {
  if (!query.trim()) return (
    <div className="empty-state">
      <div className="big-icon">🔍</div>
      <h3>File Content Search</h3>
      <p>Search the actual contents of files. Try <strong>fn_</strong>, <strong>TODO</strong>, <strong>unsafe</strong>, or <strong>RwLock</strong>.</p>
    </div>
  )
  if (matches.length === 0) return <div className="empty-state"><div className="big-icon">🔍</div><h3>No matches</h3><p>No content matches &ldquo;{query}&rdquo; — try a different pattern.</p></div>

  return <>
    <div className="result-count">{matches.length} file{matches.length !== 1 ? 's' : ''} matching &ldquo;{query}&rdquo;</div>
    {matches.map((m, i) => (
      <div key={i} className="file-item" style={{ fontFamily: 'var(--font)', flexDirection: 'column', alignItems: 'stretch' }}>
        <div className="grep-header">{m.f}:{m.l}</div>
        <pre className="grep-line"><code>{m.txt}</code></pre>
      </div>
    ))}
  </>
}

/* ─── About Panel ─── */
function AboutPanel({ overview }: { overview: OverviewData | null }) {
  return (
    <div className="about">
      <h2>FFS — Fast File Search</h2>
      <p>A high-performance file search CLI and MCP server with tree-sitter powered code navigation. Typo-resistant fuzzy matching, frecency scoring, and token-budget aware file reading.</p>

      {overview && (
        <div className="about-stats">
          <div className="about-stat"><span className="num">{overview.files}</span> files tracked</div>
          <div className="about-stat"><span className="num">{(overview.code_lines / 1000).toFixed(1)}k</span> lines of code</div>
          <div className="about-stat"><span className="num">{overview.languages.length}</span> languages</div>
        </div>
      )}

      <h3>Demo features:</h3>
      <ul>
        <li><strong>📁 Find Files</strong> — typo-tolerant fuzzy file name search</li>
        <li><strong>◎ Symbols</strong> — tree-sitter symbol definitions (functions, structs, enums, traits)</li>
        <li><strong>↗ Callers</strong> — find all call sites of a symbol</li>
        <li><strong>🔍 Grep</strong> — content search with context lines</li>
      </ul>

      <h3>Architecture</h3>
      <pre className="arch">
ffs-cli     (binary)
ffs-mcp     (MCP server)
ffs-engine  (dispatch, ranking)
ffs-grep    (SIMD search)
ffs-symbol  (tree-sitter index)
ffs-core    (scan, frecency, scoring)
      </pre>

      <p style={{ marginTop: 24 }}><a href="https://github.com/quangdang46/fast_file_search" target="_blank">GitHub →</a></p>
    </div>
  )
}
