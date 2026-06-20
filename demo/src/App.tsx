import { useState, useEffect, useRef, useCallback, type ReactElement } from 'react'
import './App.css'

/* ─── Types ─────────────────────────────────────────── */
interface OverviewData {
  files: number; code_lines: number; code_files: number; bytes: number;
  languages: { lang: string; files: number }[];
  build_files: number; entrypoints: string[]; top_symbols: { name: string; kind: string; weight: number }[];
}
interface FileData { pattern: string; matches: string[] }
interface SymHit { n: string; k: string; f: string; l: number }
interface SymData { q: string; hits: SymHit[] }
interface CallHit { f: string; l: number; txt: string; target: string; d: number }
interface CallData { q: string; hits: CallHit[] }
interface CalleeHit { n: string; f: string; l: number }
interface CalleeData { q: string; hits: CalleeHit[] }
interface GrepMatch { f: string; l: number; txt: string }
interface GrepData { q: string; matches: GrepMatch[] }
interface OutlineEntry { kind: string; name: string; start_line: number; signature?: string; children?: OutlineEntry[] }
interface OutlineData { path: string; lang: string; entries: OutlineEntry[] }
interface GlobData { pattern: string; matches: string[] }
interface ReadData { path: string; mode: string; body: string; kept_bytes: number; footer_bytes: number }
interface RefsData { name: string; definitions: RefsDef[]; usages: RefsUsage[]; total_usages: number }
interface RefsDef { path: string; line: number; end_line?: number; kind: string; weight: number; header?: string }
interface RefsUsage { path: string; line: number; text: string }
interface FlowData { name: string; cards: FlowCard[]; total_cards: number }
interface FlowCard { def: RefsDef; header: string; body: string; callees?: CalleeHit[]; callers?: CallHit[] }
interface SiblingsData { name: string; hits: SiblingHit[]; total?: number }
interface SiblingHit { name: string; kind: string; path: string; line: number; parent?: string }
interface DepsData { target: string; imports: DepImport[]; dependents: DepDependent[]; total_dependents: number }
interface DepImport { spec: string; resolved?: string }
interface DepDependent { path: string; spec: string }
interface ImpactData { name: string; results: ImpactResult[]; total: number; hops?: number }
interface ImpactResult { path: string; score: number; reasons: string[] }
interface MapData { root: string; total_files: number; total_bytes: number; total_est_tokens: number; tree: MapNode }
interface MapNode { name: string; is_dir: boolean; bytes: number; est_tokens: number; file_count: number; children: MapNode[]; truncated?: boolean; symbols?: { name: string; kind: string; line: number; weight: number }[] }

type Tab = 'files' | 'symbol' | 'callers' | 'callees' | 'grep' | 'outline' | 'glob' | 'read' | 'refs' | 'flow' | 'siblings' | 'deps' | 'impact' | 'map' | 'about'

/* ─── Helpers ───────────────────────────────────────── */
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
  const m: Record<string, string> = { rs: 'Rust', ts: 'TS', tsx: 'TSX', js: 'JS', jsx: 'JSX', py: 'Python', sh: 'Shell', toml: 'TOML', md: 'Markdown', css: 'CSS', c: 'C', h: 'C', json: 'JSON', lua: 'Lua', yaml: 'YAML', yml: 'YAML', ps1: 'PowerShell' }
  return m[ext(path)] || ext(path).toUpperCase()
}

const KIND_COLORS: Record<string, string> = {
  function_item: '#bc8cff', function: '#bc8cff',
  struct_item: '#3fb950', struct: '#3fb950',
  enum_item: '#d29922', enum: '#d29922',
  trait_item: '#f85149', trait: '#f85149',
  module: '#8b949e', import: '#8b949e',
  macro: '#f0883e', const_item: '#79c0ff',
  static_item: '#79c0ff', type_item: '#58a6ff',
}

function kindColor(kind: string): string {
  return KIND_COLORS[kind] || KIND_COLORS[kind.replace('_item', '')] || '#58a6ff'
}

function symIcon(kind: string): string {
  const k = kind.replace('_item', '')
  const icons: Record<string, string> = { function: 'ƒ', struct: 'S', enum: 'E', trait: 'T', module: 'M', macro: '⚡', const: 'c', static: 's', type: 't', import: '→' }
  return icons[k] || '?'
}

function safeKey(s: string): string {
  return s.trim().toLowerCase().replace(/[\/\.]/g, '_').replace(/[^a-z0-9_]/g, '_').replace(/_+/g, '_').replace(/^_|_$/g, '')
}

function dataPrefix(tab: Tab): string {
  const map: Partial<Record<Tab, string>> = {
    symbol: 'sym', callers: 'callers', callees: 'callees',
    grep: 'grep', outline: 'ol', glob: 'glob', read: 'read',
    refs: 'refs', flow: 'flow', siblings: 'siblings', deps: 'deps',
    impact: 'impact', 
  }
  return map[tab] || tab
}

const TAB_LABELS: Record<Tab, string> = {
  files: '📁 Find', symbol: '◎ Symbol', callers: '↗ Callers', callees: '↘ Callees',
  grep: '🔍 Grep', outline: '📋 Outline', glob: '🌐 Glob', read: '📖 Read',
  refs: '🔗 Refs', flow: '💧 Flow', siblings: '👥 Siblings', deps: '📦 Deps',
  impact: '🎯 Impact', map: '🗺 Map', about: 'ℹ About'
}

const PRESETS: Partial<Record<Tab, Array<{ label: string; key: string }>>> = {
  files: ['search', 'find', 'run', 'index', 'dispatch', 'file'].map(k => ({ label: k, key: k })),
  symbol: ['find', 'run', 'new', 'search', 'read', 'parse', 'default', 'build', 'format', 'dispatch'].map(k => ({ label: k, key: k })),
  callers: ['run', 'find', 'new', 'search', 'dispatch', 'fuzzy_search'].map(k => ({ label: k, key: k })),
  callees: ['run', 'new', 'dispatch', 'fuzzy_search'].map(k => ({ label: k, key: k })),
  grep: ['unsafe', 'RwLock', 'fn_', 'struct_', 'TODO', 'fuzzy_file_search'].map(k => ({ label: k.replace(/_$/, ' ').replace(/_/g, ' '), key: k })),
  outline: [
    { label: 'cli.rs', key: 'crates_ffs_cli_src_cli_rs' },
    { label: 'dispatch.rs', key: 'crates_ffs_engine_src_dispatch_rs' },
    { label: 'fuzzy_file_search.rs', key: 'crates_ffs_core_src_fuzzy_file_search_rs' },
    { label: 'symbol_index.rs', key: 'crates_ffs_symbol_src_symbol_index_rs' },
    { label: 'file_picker.rs', key: 'crates_ffs_core_src_file_picker_rs' },
  ],
  glob: ['*.rs', '*.toml', '*.md', '*.sh', '*.json', '*.c', '*.py'].map(k => ({ label: k, key: 'star_' + k.slice(2).replace('.', '_') })),
  read: [
    { label: 'cli.rs', key: 'crates_ffs_cli_src_cli_rs' },
    { label: 'dispatch.rs', key: 'crates_ffs_engine_src_dispatch_rs' },
    { label: 'lib.rs', key: 'crates_ffs_engine_src_lib_rs' },
    { label: 'main.rs', key: 'crates_ffs_cli_src_main_rs' },
    { label: 'fuzzy_file_search.rs', key: 'crates_ffs_core_src_fuzzy_file_search_rs' },
    { label: 'mod.rs', key: 'crates_ffs_cli_src_commands_mod_rs' },
  ],
  refs: ['find', 'run', 'search', 'dispatch', 'read', 'new', 'build', 'parse'].map(k => ({ label: k, key: k })),
  flow: ['find', 'run', 'search', 'dispatch', 'read', 'new'].map(k => ({ label: k, key: k })),
  siblings: ['find', 'run', 'read', 'new', 'search'].map(k => ({ label: k, key: k })),
  deps: [
    { label: 'cli.rs', key: 'crates_ffs_cli_src_cli_rs' },
    { label: 'dispatch.rs', key: 'crates_ffs_engine_src_dispatch_rs' },
    { label: 'fuzzy_file_search.rs', key: 'crates_ffs_core_src_fuzzy_file_search_rs' },
  ],
  impact: ['find', 'run', 'search', 'dispatch', 'read'].map(k => ({ label: k, key: k })),

  map: [],
}

async function fetchJSON<T>(url: string): Promise<T | null> {
  try { const r = await fetch(url); if (!r.ok) return null; return await r.json() as T }
  catch { return null }
}

/* ══════════════════════════════════════════════════════ */
export default function App() {
  const [tab, setTab] = useState<Tab>('files')
  const [overview, setOverview] = useState<OverviewData | null>(null)

  // Shared
  const [files, setFiles] = useState<string[]>([])
  const [q, setQ] = useState('')
  const [results, setResults] = useState<Array<{ path: string; score: number }>>([])

  // Tab state
  const [symQ, setSymQ] = useState(''); const [symResults, setSymResults] = useState<SymHit[]>([])
  const [callerQ, setCallerQ] = useState(''); const [callerResults, setCallerResults] = useState<CallHit[]>([])
  const [calleeQ, setCalleeQ] = useState(''); const [calleeResults, setCalleeResults] = useState<CalleeHit[]>([])
  const [grepQ, setGrepQ] = useState(''); const [grepResults, setGrepResults] = useState<GrepMatch[]>([])
  const [outlineFile, setOutlineFile] = useState(''); const [outlineData, setOutlineData] = useState<OutlineData | null>(null)
  const [globQ, setGlobQ] = useState(''); const [globData, setGlobData] = useState<GlobData | null>(null)
  const [readQ, setReadQ] = useState(''); const [readData, setReadData] = useState<ReadData | null>(null)
  const [refsQ, setRefsQ] = useState(''); const [refsData, setRefsData] = useState<RefsData | null>(null)
  const [flowQ, setFlowQ] = useState(''); const [flowData, setFlowData] = useState<FlowData | null>(null)
  const [siblingsQ, setSiblingsQ] = useState(''); const [siblingsData, setSiblingsData] = useState<SiblingsData | null>(null)
  const [depsQ, setDepsQ] = useState(''); const [depsData, setDepsData] = useState<DepsData | null>(null)
  const [impactQ, setImpactQ] = useState(''); const [impactData, setImpactData] = useState<ImpactData | null>(null)
    const [mapData, setMapData] = useState<MapData | null>(null)

  // Detail panel
  const [detail, setDetail] = useState<{ title: string; modeTag: string; body: ReactElement }>({
    title: 'Ready', modeTag: 'FFS', body: <div className="detail-empty">Type a query or click a preset to see results and details</div>
  })

  const inputRef = useRef<HTMLInputElement>(null)

  /* Load static */
  useEffect(() => {
    Promise.all([
      fetchJSON<FileData>('/data/files.json'),
      fetchJSON<OverviewData>('/data/overview.json'),
      fetchJSON<MapData>('/data/map.json'),
    ]).then(([fd, ov, md]) => {
      if (fd) { setFiles(fd.matches); setResults(fd.matches.map(p => ({ path: p, score: 0 }))) }
      if (ov) setOverview(ov)
      if (md) setMapData(md)
    })
  }, [])

  /* Generic data loader */
  const loadData = useCallback(async (t: Tab, query: string) => {
    const prefix = dataPrefix(t); const key = safeKey(query)
    if (!key) return
    const data = await fetchJSON<any>(`/data/${prefix}_${key}.json`)
    if (!data) return
    const t2 = t as string
    if (t2 === 'symbol') { const d = data as SymData; setSymResults(d.hits || []); autoSelectFirstSym(d.hits, query) }
    else if (t2 === 'callers') { const d = data as CallData; setCallerResults(d.hits || []); autoSelectFirstCaller(d.hits) }
    else if (t2 === 'callees') { const d = data as CalleeData; setCalleeResults(d.hits || []); autoSelectFirstCallee(d.hits) }
    else if (t2 === 'grep') { const d = data as GrepData; setGrepResults(d.matches || []); autoSelectFirstGrep(d.matches) }
    else if (t2 === 'glob') { setGlobData(data as GlobData) }
    else if (t2 === 'refs') { const d = data as RefsData; setRefsData(d) }
    else if (t2 === 'flow') { const d = data as FlowData; setFlowData(d) }
    else if (t2 === 'siblings') { const d = data as SiblingsData; setSiblingsData(d) }
    else if (t2 === 'impact') { const d = data as ImpactData; setImpactData(d) }
  }, [])

  /* ─── Outline loader ─── */
  const loadOutline = useCallback(async (file: string) => {
    if (!file.trim()) { setOutlineData(null); return }
    const key = safeKey(file)
    let data = await fetchJSON<OutlineData>(`/data/ol_${key}.json`)
    if (!data) {
      const allFiles = ['crates_ffs_cli_src_cli_rs', 'crates_ffs_core_src_file_picker_rs', 'crates_ffs_core_src_fuzzy_file_search_rs', 'crates_ffs_engine_src_dispatch_rs', 'crates_ffs_symbol_src_symbol_index_rs']
      const match = allFiles.find(f => f.includes(key))
      if (match) data = await fetchJSON<OutlineData>(`/data/ol_${match}.json`)
    }
    setOutlineData(data)
    if (data) setDetail({ title: data.path.split('/').pop() || '', modeTag: data.lang, body: <OutlineDetail data={data} /> })
  }, [])

  /* ─── Read loader ─── */
  const loadRead = useCallback(async (file: string) => {
    if (!file.trim()) { setReadData(null); return }
    const key = safeKey(file)
    let data = await fetchJSON<ReadData>(`/data/read_${key}.json`)
    if (!data) {
      const allFiles = ['crates_ffs_cli_src_cli_rs', 'crates_ffs_engine_src_dispatch_rs', 'crates_ffs_core_src_fuzzy_file_search_rs', 'crates_ffs-core_src_file_picker_rs', 'crates_ffs-symbol_src_symbol_index_rs', 'crates_ffs-cli_src_main_rs', 'crates_ffs-engine_src_lib_rs', 'crates_ffs-cli_src_commands_mod_rs']
      const match = allFiles.find(f => f.includes(key))
      if (match) data = await fetchJSON<ReadData>(`/data/read_${match}.json`)
    }
    setReadData(data)
    if (data) setDetail({ title: data.path.split('/').pop() || '', modeTag: `read ${data.mode}`, body: <ReadDetail data={data} /> })
    else setDetail({ title: file, modeTag: 'read', body: <div className="detail-empty">No content available for this file</div> })
  }, [])

  /* ─── Deps loader ─── */
  const loadDeps = useCallback(async (file: string) => {
    if (!file.trim()) { setDepsData(null); return }
    const key = safeKey(file)
    let data = await fetchJSON<DepsData>(`/data/deps_${key}.json`)
    if (!data) {
      const allFiles = ['crates_ffs_cli_src_cli_rs', 'crates_ffs_engine_src_dispatch_rs', 'crates_ffs_core_src_fuzzy_file_search_rs']
      const match = allFiles.find(f => f.includes(key))
      if (match) data = await fetchJSON<DepsData>(`/data/deps_${match}.json`)
    }
    setDepsData(data)
    if (data) setDetail({ title: data.target.split('/').pop() || '', modeTag: 'deps', body: <DepsDetail data={data} /> })
  }, [])

  /* ─── Dispatch loader ─── */
  /* ─── Auto-select first result for detail view ─── */
  const autoSelectFirstFile = useCallback(async (paths: string[], _query: string) => {
    if (paths.length === 0) return
    const path = typeof paths[0] === 'string' ? paths[0] : paths[0]
    const key = safeKey(path)
    const data = await fetchJSON<ReadData>(`/data/read_${key}.json`)
    if (data) setDetail({ title: path, modeTag: 'file', body: <ReadDetail data={data} /> })
    else setDetail({ title: path, modeTag: 'file', body: <div className="detail-info"><p>File: {path}</p><p>Extension: .{ext(path)}</p><p>Language: {langBadge(path)}</p></div> })
  }, [])

  const autoSelectFirstSym = useCallback((hits: SymHit[], _query: string) => {
    if (hits.length === 0) return
    setDetail({
      title: `Symbol: ${_query}`, modeTag: 'symbol',
      body: <div>{hits.slice(0, 15).map((h, i) => (
        <div key={i} className="detail-sym-card">
          <div className="detail-sym-header">
            <span className="sym-kind" style={{ background: `${kindColor(h.k)}22`, color: kindColor(h.k), borderColor: `${kindColor(h.k)}44` }}>{symIcon(h.k)}</span>
            <span className="detail-sym-name">{h.n}</span>
            <span className="detail-sym-loc">{h.f.split('/').pop()} L{h.l}</span>
            <span className="lang" style={{ marginLeft: 'auto' }}>{h.k.replace('_item', '')}</span>
          </div>
        </div>
      ))}</div>
    })
  }, [])

  const autoSelectFirstCaller = useCallback((hits: CallHit[]) => {
    if (hits.length === 0) return
    setDetail({
      title: `Callers (${hits.length})`, modeTag: 'callers',
      body: <div>{hits.slice(0, 20).map((h, i) => (
        <div key={i} className="detail-match-line">
          <span className="detail-match-num">L{h.l}</span>
          <span className="detail-match-text">{h.txt}</span>
        </div>
      ))}</div>
    })
  }, [])

  const autoSelectFirstCallee = useCallback((hits: CalleeHit[]) => {
    if (hits.length === 0) return
    const byName: Record<string, { name: string; files: string[] }> = {}
    for (const h of hits) { if (!byName[h.n]) byName[h.n] = { name: h.n, files: [] }; if (!byName[h.n].files.includes(h.f)) byName[h.n].files.push(h.f) }
    const top = Object.values(byName).sort((a, b) => b.files.length - a.files.length).slice(0, 20)
    setDetail({
      title: `Callees (${hits.length})`, modeTag: 'callees',
      body: <div>{top.map((sym, i) => (
        <div key={i} className="detail-sym-card">
          <div className="detail-sym-header">
            <span className="sym-kind">↘</span>
            <span className="detail-sym-name">{sym.name}</span>
            <span className="detail-sym-loc">{sym.files.length} file{sym.files.length !== 1 ? 's' : ''}</span>
          </div>
        </div>
      ))}</div>
    })
  }, [])

  const autoSelectFirstGrep = useCallback((matches: GrepMatch[]) => {
    if (matches.length === 0) return
    const byFile: Record<string, GrepMatch[]> = {}
    for (const m of matches) { if (!byFile[m.f]) byFile[m.f] = []; byFile[m.f].push(m) }
    const firstFile = Object.entries(byFile)[0]
    if (!firstFile) return
    setDetail({
      title: firstFile[0], modeTag: 'grep',
      body: <div>{firstFile[1].slice(0, 30).map((m, i) => (
        <div key={i} className="detail-match-line">
          <span className="detail-match-num">L{m.l}</span>
          <span className="detail-match-text">{m.txt}</span>
        </div>
      ))}</div>
    })
  }, [])

  const autoSelectFirstGlob = useCallback(async (data: GlobData) => {
    if (!data.matches?.length) return
    await autoSelectFirstFile(data.matches, '')
  }, [autoSelectFirstFile])

  /* When tab or search results change, auto-update detail */
  useEffect(() => { if (results.length > 0 && !q.trim()) autoSelectFirstFile(results.map(r => r.path), '') }, [results.length])
  useEffect(() => { if (globData && globData.matches?.length) autoSelectFirstGlob(globData) }, [globData])
  useEffect(() => { if (refsData) setDetail({ title: `Refs: ${refsData.name}`, modeTag: 'refs', body: <RefsDetail data={refsData} /> }) }, [refsData])
  useEffect(() => { if (flowData) setDetail({ title: `Flow: ${flowData.name}`, modeTag: 'flow', body: <FlowDetail data={flowData} /> }) }, [flowData])
  useEffect(() => { if (siblingsData) setDetail({ title: `Siblings: ${siblingsData.name}`, modeTag: 'siblings', body: <SiblingsDetail data={siblingsData} /> }) }, [siblingsData])
  useEffect(() => { if (impactData) setDetail({ title: `Impact: ${impactData.name}`, modeTag: 'impact', body: <ImpactDetail data={impactData} /> }) }, [impactData])
  useEffect(() => { if (depsData) setDetail({ title: depsData.target.split('/').pop() || '', modeTag: 'deps', body: <DepsDetail data={depsData} /> }) }, [depsData])
  /* On files search, auto-select first match */
  useEffect(() => {
    if (results.length > 0 && q.trim()) autoSelectFirstFile(results.map(r => r.path), q)
  }, [q, results.length])

  /* ─── Tab switch ─── */
  const switchTab = useCallback((t: Tab) => {
    setTab(t)
    // Reset detail for tabs that don't auto-populate
    if (t === 'map' || t === 'about' || t === 'outline') {
      setDetail({ title: 'Ready', modeTag: TAB_LABELS[t].split(' ')[0], body: <div className="detail-empty">Select an item on the left to see details</div> })
    }
    setTimeout(() => inputRef.current?.focus(), 50)
  }, [])

  /* ─── File click → detail ─── */
  const onFileClick = useCallback(async (path: string) => {
    const key = safeKey(path)
    const data = await fetchJSON<ReadData>(`/data/read_${key}.json`)
    if (data) setDetail({ title: path, modeTag: 'file', body: <ReadDetail data={data} /> })
    else setDetail({ title: path, modeTag: 'file', body: <div className="detail-info"><p>File: {path}</p><p>Extension: .{ext(path)}</p><p>Language: {langBadge(path)}</p></div> })
  }, [])

  /* ─── Preset click ─── */
  const onPreset = useCallback((t2: string, key: string) => {
    if (t2 === 'files') { setQ(key); setResults(files.filter(f => fuzzyScore(key, f) > 0).map(f => ({ path: f, score: fuzzyScore(key, f) })).sort((a, b) => b.score - a.score).slice(0, 100)) }
    else if (t2 === 'symbol') { setSymQ(key); loadData(tab, key) }
    else if (t2 === 'callers') { setCallerQ(key); loadData(tab, key) }
    else if (t2 === 'callees') { setCalleeQ(key); loadData(tab, key) }
    else if (t2 === 'grep') { setGrepQ(key); loadData(tab, key) }
    else if (t2 === 'glob') { setGlobQ(key); loadData(tab, key) }
    else if (t2 === 'outline') { setOutlineFile(key); loadOutline(key) }
    else if (t2 === 'read') { setReadQ(key); loadRead(key) }
    else if (t2 === 'refs') { setRefsQ(key); loadData(tab, key) }
    else if (t2 === 'flow') { setFlowQ(key); loadData(tab, key) }
    else if (t2 === 'siblings') { setSiblingsQ(key); loadData(tab, key) }
    else if (t2 === 'deps') { setDepsQ(key); loadDeps(key) }
    else if (t2 === 'impact') { setImpactQ(key); loadData(tab, key) }
  }, [files, tab, loadData, loadOutline, loadRead, loadDeps])

  return (
    <div className="app">
      <header className="header">
        <div className="logo">
          <span className="logo-icon">⚡</span>
          <h1>FFS</h1>
        </div>
        <span className="badge">v0.8</span>
        <span className="subtitle">Fast File Search</span>
        {overview && (
          <div className="header-stats">
            <span className="stat"><span className="num">{overview.files}</span> files</span>
            <span className="stat"><span className="num">{(overview.code_lines / 1000).toFixed(1)}k</span> lines</span>
            <span className="stat"><span className="num">{overview.languages.length}</span> langs</span>
          </div>
        )}
      </header>

      <nav className="tabs">
        {(['files','symbol','callers','callees','grep','outline','glob','read','refs','flow','siblings','deps','impact','dispatch','map','about'] as Tab[]).map(t => (
          <button key={t} className={`tab${tab === t ? ' active' : ''}`} onClick={() => switchTab(t)}>{TAB_LABELS[t]}</button>
        ))}
      </nav>

      {/* Search bar */}
      {(tab as string !== 'about' && tab as string !== 'map') && (
        <div className="search-wrap">
          {(['files','symbol','callers','callees','grep','glob','refs','flow','siblings','impact'] as string[]).includes(tab) && (
            <div className="search-bar">
              <svg className="icon" width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M6.5 2a4.5 4.5 0 1 0 0 9 4.5 4.5 0 0 0 0-9zM11 11l3 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
              </svg>
              <input ref={inputRef} type="text" className="search-input"
                placeholder={tab === 'files' ? 'Search files...' : tab === 'symbol' ? 'Symbol name...' : tab === 'callers' ? 'Find callers of...' : tab === 'callees' ? 'Find callees of...' : tab === 'grep' ? 'Content search...' : tab === 'glob' ? 'Glob pattern...' : tab === 'refs' ? 'Symbol name...' : tab === 'flow' ? 'Symbol name...' : tab === 'siblings' ? 'Symbol name...' : tab === 'impact' ? 'Symbol name...' : 'Search...'}
                value={tab === 'files' ? q : tab === 'symbol' ? symQ : tab === 'callers' ? callerQ : tab === 'callees' ? calleeQ : tab === 'grep' ? grepQ : tab === 'glob' ? globQ : tab === 'refs' ? refsQ : tab === 'flow' ? flowQ : tab === 'siblings' ? siblingsQ : tab === 'impact' ? impactQ : q}
                onChange={e => { const v = e.target.value; const t = tab as string;
                  if (t === 'files') { setQ(v); const r = v ? files.filter(f => fuzzyScore(v, f) > 0).map(f => ({ path: f, score: fuzzyScore(v, f) })).sort((a, b) => b.score - a.score).slice(0, 100) : files.map(f => ({ path: f, score: 0 })); setResults(r); if (r.length > 0) onFileClick(r[0].path); }
                  else if (t === 'symbol') { setSymQ(v); if (v.trim()) loadData(tab, v.trim()); else setSymResults([]); }
                  else if (t === 'callers') { setCallerQ(v); if (v.trim()) loadData(tab, v.trim()); else setCallerResults([]); }
                  else if (t === 'callees') { setCalleeQ(v); if (v.trim()) loadData(tab, v.trim()); else setCalleeResults([]); }
                  else if (t === 'grep') { setGrepQ(v); if (v.trim()) loadData(tab, v.trim()); else setGrepResults([]); }
                  else if (t === 'glob') { setGlobQ(v); const gk = v.trim().replace(/\*/g, 'star').replace(/\./g, '_'); if (gk) loadData(tab, gk); else setGlobData(null); }
                  else if (t === 'refs') { setRefsQ(v); if (v.trim()) loadData(tab, v.trim()); else setRefsData(null); }
                  else if (t === 'flow') { setFlowQ(v); if (v.trim()) loadData(tab, v.trim()); else setFlowData(null); }
                  else if (t === 'siblings') { setSiblingsQ(v); if (v.trim()) loadData(tab, v.trim()); else setSiblingsData(null); }
                  else if (t === 'impact') { setImpactQ(v); if (v.trim()) loadData(tab, v.trim()); else setImpactData(null); }
                }}
              />
            </div>
          )}
          {(tab as string) === 'outline' && (
            <div className="search-bar">
              <span className="icon">📋</span>
              <input ref={inputRef} type="text" className="search-input" placeholder="File name (e.g. cli.rs)" value={outlineFile}
                onChange={e => { setOutlineFile(e.target.value); if (e.target.value.trim()) loadOutline(e.target.value.trim()); }} />
            </div>
          )}
          {(tab as string) === 'read' && (
            <div className="search-bar">
              <span className="icon">📖</span>
              <input ref={inputRef} type="text" className="search-input" placeholder="File path (e.g. crates/ffs-cli/src/cli.rs)" value={readQ}
                onChange={e => setReadQ(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter' && readQ.trim()) loadRead(readQ.trim()); }} />
            </div>
          )}
          {(tab as string) === 'deps' && (
            <div className="search-bar">
              <span className="icon">📦</span>
              <input ref={inputRef} type="text" className="search-input" placeholder="File path (e.g. crates/ffs-cli/src/cli.rs)" value={depsQ}
                onChange={e => setDepsQ(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter' && depsQ.trim()) loadDeps(depsQ.trim()); }} />
            </div>
          )}

          <div className="search-meta">
            <div className="presets">
              {PRESETS[tab]?.slice(0, 8).map(p => (
                <button key={p.key} className="preset-btn" onClick={() => onPreset(tab as string, p.key)}>{p.label}</button>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* Split layout: results + detail */}
      <div className="main-area">
        <div className="results-panel">
          <main className="results">
            {tab === 'files' && <FilesPanel query={q} results={results} onFileClick={onFileClick} />}
            {tab === 'symbol' && <SymbolPanel query={symQ} hits={symResults} />}
            {tab === 'callers' && <CallersPanel query={callerQ} hits={callerResults} />}
            {tab === 'callees' && <CalleesPanel query={calleeQ} hits={calleeResults} />}
            {tab === 'grep' && <GrepPanel query={grepQ} matches={grepResults} onFileClick={onFileClick} />}
            {tab === 'outline' && <OutlinePanel data={outlineData} file={outlineFile} />}
            {tab === 'glob' && <GlobPanel pattern={globQ} data={globData} onFileClick={onFileClick} />}
            {tab === 'read' && <ReadPanel data={readData} file={readQ} />}
            {tab === 'refs' && <RefsPanel query={refsQ} data={refsData} onFileClick={onFileClick} />}
            {tab === 'flow' && <FlowPanel query={flowQ} data={flowData} />}
            {tab === 'siblings' && <SiblingsPanel query={siblingsQ} data={siblingsData} />}
            {tab === 'deps' && <DepsPanel file={depsQ} data={depsData} />}
            {tab === 'impact' && <ImpactPanel query={impactQ} data={impactData} />}
                        {tab === 'map' && <MapPanel data={mapData} />}
            {tab === 'about' && <AboutPanel overview={overview} />}
          </main>
        </div>

        {/* Detail panel — always visible */}
        <aside className="detail-panel">
          <div className="detail-header">
            <div className="detail-header-left">
              <span className="detail-mode-tag">{detail.modeTag}</span>
              <span className="detail-title">{detail.title}</span>
            </div>
          </div>
          <div className="detail-body-wrap">
            {detail.body}
          </div>
        </aside>
      </div>
    </div>
  )
}

/* ═══ Panels ═══ */

/* ─── Files ─── */
function FilesPanel({ query, results, onFileClick }: { query: string; results: Array<{ path: string; score: number }>; onFileClick: (path: string) => void }) {
  if (!query.trim()) return emptyState('⚡', 'Find Files', 'Typo-resistant fuzzy file name search.', 'search', 'find', 'run')
  if (results.length === 0) return emptyResult('No files found', `No files matching "${query}"`)
  return (<>
    <div className="result-count">{results.length} file{results.length !== 1 ? 's' : ''} matching &ldquo;{query}&rdquo;</div>
    {results.map((r, i) => (
      <div key={i} className="file-item clickable" onClick={() => onFileClick(r.path)}>
        <span className="ext">{ext(r.path)}</span>
        <span className="path">{highlight(r.path, query)}</span>
        <span className="lang">{langBadge(r.path)}</span>
        {r.score > 0 && <span className="score">{r.score}</span>}
      </div>
    ))}
  </>)
}

/* ─── Symbol ─── */
function SymbolPanel({ query, hits }: { query: string; hits: SymHit[] }) {
  const byName: Record<string, { name: string; kind: string; files: string[] }> = {}
  for (const h of hits) { if (!byName[h.n]) byName[h.n] = { name: h.n, kind: h.k, files: [] }; if (!byName[h.n].files.includes(h.f)) byName[h.n].files.push(h.f) }
  if (!query.trim()) return emptyState('◎', 'Symbol Lookup', 'Tree-sitter powered symbol definitions.', 'find', 'run', 'new')
  if (hits.length === 0) return emptyResult('No symbols', `No symbols for "${query}"`)
  const top = Object.values(byName).sort((a, b) => b.files.length - a.files.length).slice(0, 40)
  return (<>
    <div className="result-count">{hits.length} def{hits.length !== 1 ? 's' : ''} of &ldquo;{query}&rdquo; ({top.length} unique)</div>
    {top.map((sym, i) => (
      <div key={i} className="file-item">
        <span className="sym-kind" style={{ background: `${kindColor(sym.kind)}22`, color: kindColor(sym.kind), borderColor: `${kindColor(sym.kind)}44` }}>{symIcon(sym.kind)}</span>
        <span className="path"><strong>{sym.name}</strong></span>
        <span className="lang">{sym.kind.replace('_item', '').replace('_', ' ')}</span>
        <span style={{ fontSize: 11, color: 'var(--text-muted)', marginLeft: 'auto' }}>{sym.files.length} file{sym.files.length !== 1 ? 's' : ''}</span>
      </div>
    ))}
  </>)
}

/* ─── Callers ─── */
function CallersPanel({ query, hits }: { query: string; hits: CallHit[] }) {
  if (!query.trim()) return emptyState('↗', 'Callers', 'Find all call sites of a symbol.', 'run', 'find', 'new')
  if (hits.length === 0) return emptyResult('No callers', `No callers of "${query}"`)
  return (<>
    <div className="result-count">{hits.length} caller{hits.length !== 1 ? 's' : ''} of &ldquo;{query}&rdquo;</div>
    {hits.map((h, i) => (
      <div key={i} className="file-item" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 2, padding: '5px 10px' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
          <span className="call-line">L{h.l}</span>
          <span className="path" style={{ fontSize: 12 }}>{h.f}</span>
          <span className="lang">{langBadge(h.f)}</span>
        </div>
        <code className="call-text">{h.txt}</code>
      </div>
    ))}
  </>)
}

/* ─── Callees ─── */
function CalleesPanel({ query, hits }: { query: string; hits: CalleeHit[] }) {
  const byName: Record<string, { name: string; files: string[] }> = {}
  for (const h of hits) { if (!byName[h.n]) byName[h.n] = { name: h.n, files: [] }; if (!byName[h.n].files.includes(h.f)) byName[h.n].files.push(h.f) }
  if (!query.trim()) return emptyState('↘', 'Callees', 'See what symbols a function calls.', 'run', 'new', 'fuzzy_search')
  if (hits.length === 0) return emptyResult('No callees', `No callees for "${query}"`)
  const top = Object.values(byName).sort((a, b) => b.files.length - a.files.length).slice(0, 30)
  return (<>
    <div className="result-count">{hits.length} callee{hits.length !== 1 ? 's' : ''} ({top.length} unique)</div>
    {top.map((sym, i) => (
      <div key={i} className="file-item">
        <span className="sym-kind">↘</span>
        <span className="path"><strong>{sym.name}</strong></span>
        <span className="lang" style={{ marginLeft: 'auto', fontSize: 11 }}>{sym.files.length} file{sym.files.length !== 1 ? 's' : ''}</span>
      </div>
    ))}
  </>)
}

/* ─── Grep ─── */
function GrepPanel({ query, matches, onFileClick }: { query: string; matches: GrepMatch[]; onFileClick: (path: string) => void }) {
  if (!query.trim()) return emptyState('🔍', 'Content Search', 'SIMD-accelerated content grep.', 'unsafe', 'RwLock', 'TODO')
  if (matches.length === 0) return emptyResult('No matches', `No content matches for "${query}"`)
  const byFile: Record<string, GrepMatch[]> = {}
  for (const m of matches) { if (!byFile[m.f]) byFile[m.f] = []; byFile[m.f].push(m) }
  const files = Object.keys(byFile)
  return (<>
    <div className="result-count">{matches.length} match{matches.length !== 1 ? 'es' : ''} across {files.length} file{files.length !== 1 ? 's' : ''}</div>
    {files.map((f, i) => (
      <div key={i} style={{ marginBottom: 10 }}>
        <div className="grep-header clickable" onClick={() => onFileClick(f)}>📄 {f.split('/').pop()} · {byFile[f].length} match{byFile[f].length !== 1 ? 'es' : ''}</div>
        {byFile[f].slice(0, 5).map((m, j) => (
          <div key={j} className="grep-line">
            <span className="call-line">L{m.l}</span>
            <code className="grep-text">{m.txt}</code>
          </div>
        ))}
        {byFile[f].length > 5 && <div style={{ fontSize: 11, color: 'var(--text-muted)', padding: '2px 10px' }}>… {byFile[f].length - 5} more</div>}
      </div>
    ))}
  </>)
}

/* ─── Outline ─── */
function OutlineEntryDisplay({ entry, depth }: { entry: OutlineEntry; depth: number }) {
  const [open, setOpen] = useState(depth < 1)
  const hasChildren = entry.children && entry.children.length > 0
  return (<>
    <div className="ol-entry" style={{ paddingLeft: 12 + depth * 16 }} onClick={() => setOpen(!open)}>
      {hasChildren ? <span className="ol-toggle">{open ? '▼' : '▶'}</span> : <span className="ol-toggle" style={{ visibility: 'hidden' }}>▶</span>}
      <span className={`ol-kind ol-kind-${entry.kind.replace('_item', '')}`}>{symIcon(entry.kind)}</span>
      <span className="ol-name">{entry.name === '<anonymous>' ? '(anon)' : entry.name}</span>
      <span className="ol-line">L{entry.start_line}</span>
    </div>
    {open && hasChildren && entry.children!.map((c, i) => <OutlineEntryDisplay key={i} entry={c} depth={depth + 1} />)}
  </>)
}

function OutlinePanel({ data, file }: { data: OutlineData | null; file: string }) {
  if (!file) return emptyState('📋', 'File Outline', 'View structural outline — functions, structs, enums.', 'cli.rs', 'dispatch.rs')
  if (!data) return emptyResult('No outline', `No outline data for "${file}"`)
  return (<>
    <div className="result-count">{data.path.split('/').pop()} · {data.lang} · {data.entries.length} top-level</div>
    {data.entries.map((e, i) => <OutlineEntryDisplay key={i} entry={e} depth={0} />)}
  </>)
}

/* ─── Glob ─── */
function GlobPanel({ pattern, data, onFileClick }: { pattern: string; data: GlobData | null; onFileClick: (path: string) => void }) {
  if (!pattern.trim()) return emptyState('🌐', 'Glob Search', 'Match files by glob patterns.', '*.rs', '*.toml', '*.md')
  if (!data || !data.matches.length) return emptyResult('No matches', `No files match "${pattern}"`)
  return (<>
    <div className="result-count">{data.matches.length} file{data.matches.length !== 1 ? 's' : ''} matching <code>{pattern}</code></div>
    {data.matches.map((m, i) => (
      <div key={i} className="file-item clickable" onClick={() => onFileClick(m)}>
        <span className="ext">{ext(m)}</span>
        <span className="path">{m}</span>
        <span className="lang">{langBadge(m)}</span>
      </div>
    ))}
  </>)
}

/* ─── Read ─── */
function ReadPanel({ data, file }: { data: ReadData | null; file: string }) {
  if (!file.trim()) return emptyState('📖', 'Token-Aware Read', 'Read files with token-budget aware truncation.', 'cli.rs', 'dispatch.rs', 'lib.rs')
  if (!data) return emptyResult('Not available', `No data for "${file}". Try a preset.`)
  return (<>
    <div className="result-count">📄 {data.path.split('/').pop()} · <kbd>{data.mode}</kbd> · {data.kept_bytes}B · {data.footer_bytes > 0 ? `${data.footer_bytes} truncated` : 'full'}</div>
    <pre className="read-body" style={{ padding: '12px 10px' }}>{data.body}</pre>
  </>)
}

/* ─── Refs ─── */
function RefsPanel({ query, data, onFileClick }: { query: string; data: RefsData | null; onFileClick: (path: string) => void }) {
  if (!query.trim()) return emptyState('🔗', 'Refs', 'Definitions + usages in one shot.', 'find', 'run', 'search')
  if (!data) return emptyResult('No refs', `No refs for "${query}"`)
  const defs = data.definitions || []; const uses = data.usages || []
  return (<>
    <div className="result-count">&ldquo;{data.name}&rdquo; · {defs.length} def{defs.length !== 1 ? 's' : ''} · {data.total_usages} usage{data.total_usages !== 1 ? 's' : ''}</div>
    {defs.length > 0 && (<>
      <div className="section-title">Definitions</div>
      {defs.slice(0, 10).map((d, i) => (
        <div key={i} className="file-item clickable" onClick={() => onFileClick(d.path)}>
          <span className="sym-kind" style={{ background: `${kindColor(d.kind)}22`, color: kindColor(d.kind), borderColor: `${kindColor(d.kind)}44` }}>{symIcon(d.kind)}</span>
          <span className="path" style={{ fontSize: 12 }}>{d.path?.split('/').slice(-2).join('/')}</span>
          <span className="call-line">L{d.line}</span>
          <span className="lang">{d.kind.replace('_item', '')}</span>
        </div>
      ))}
    </>)}
    {uses.length > 0 && (<>
      <div className="section-title">Usages ({uses.length})</div>
      {uses.slice(0, 20).map((u, i) => (
        <div key={i} className="file-item" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 2 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
            <span className="call-line">L{u.line}</span>
            <span className="path" style={{ fontSize: 12 }}>{u.path?.split('/').slice(-3).join('/')}</span>
          </div>
          <code className="call-text">{u.text}</code>
        </div>
      ))}
    </>)}
  </>)
}

/* ─── Flow ─── */
function FlowPanel({ query, data }: { query: string; data: FlowData | null }) {
  if (!query.trim()) return emptyState('💧', 'Flow', 'Drill-down: def + body + callees + callers.', 'find', 'run', 'search')
  if (!data || !data.cards?.length) return emptyResult('No flow', `No flow for "${query}"`)
  return (<>
    <div className="result-count">{data.cards.length} card{data.cards.length !== 1 ? 's' : ''} for &ldquo;{data.name}&rdquo;</div>
    {data.cards.slice(0, 10).map((card, i) => (
      <div key={i} className="flow-card">
        <div className="flow-card-header">
          <span className="sym-kind" style={{ background: `${kindColor(card.def.kind)}22`, color: kindColor(card.def.kind), borderColor: `${kindColor(card.def.kind)}44` }}>{symIcon(card.def.kind)}</span>
          <span className="path"><strong>{card.def.path?.split('/').pop()}</strong></span>
          <span className="call-line">L{card.def.line}</span>
          <span className="lang">{card.def.kind.replace('_item', '')}</span>
        </div>
        <pre className="flow-card-body"><code>{card.body?.slice(0, 250)}{card.body && card.body.length > 250 ? '…' : ''}</code></pre>
        <div className="flow-card-footer">
          {card.callees && <span>{card.callees.length} callees</span>}
          {card.callers && <span>{card.callers.length} callers</span>}
        </div>
      </div>
    ))}
  </>)
}

/* ─── Siblings ─── */
function SiblingsPanel({ query, data }: { query: string; data: SiblingsData | null }) {
  if (!query.trim()) return emptyState('👥', 'Siblings', 'Peer symbols in the same parent scope.', 'find', 'run', 'read')
  if (!data || !data.hits?.length) return emptyResult('No siblings', `No siblings for "${query}"`)
  const byFile: Record<string, SiblingHit[]> = {}
  for (const h of data.hits) { if (!byFile[h.path]) byFile[h.path] = []; byFile[h.path].push(h) }
  return (<>
    <div className="result-count">{data.hits.length} sibling{data.hits.length !== 1 ? 's' : ''} across {Object.keys(byFile).length} file{Object.keys(byFile).length !== 1 ? 's' : ''}</div>
    {Object.entries(byFile).slice(0, 10).map(([path, syms], i) => (
      <div key={i} style={{ marginBottom: 6 }}>
        <div className="grep-header">📄 {path.split('/').slice(-3).join('/')}</div>
        {syms.slice(0, 10).map((s, j) => (
          <div key={j} className="file-item">
            <span className="sym-kind" style={{ background: `${kindColor(s.kind)}22`, color: kindColor(s.kind), borderColor: `${kindColor(s.kind)}44` }}>{symIcon(s.kind)}</span>
            <span className="path">{s.name}</span>
            <span className="call-line">L{s.line}</span>
            <span className="lang">{s.kind}</span>
          </div>
        ))}
      </div>
    ))}
  </>)
}

/* ─── Deps ─── */
function DepsPanel({ file, data }: { file: string; data: DepsData | null }) {
  if (!file.trim()) return emptyState('📦', 'Dependencies', 'File imports + workspace dependents.', 'cli.rs', 'dispatch.rs')
  if (!data) return emptyResult('No deps', `No deps for "${file}". Try a preset.`)
  const imports = data.imports || []; const dependents = data.dependents || []
  return (<>
    <div className="result-count">{data.target.split('/').pop()} · {imports.length} import{imports.length !== 1 ? 's' : ''} · {data.total_dependents} dependent{data.total_dependents !== 1 ? 's' : ''}</div>
    {imports.length > 0 && (<>
      <div className="section-title">Imports</div>
      {imports.map((imp, i) => (
        <div key={i} className="file-item">
          <span className="call-line" style={{ minWidth: 20 }}>{i + 1}</span>
          <span className="path" style={{ fontSize: 12 }}>{imp.spec}</span>
          {imp.resolved && <span className="lang" style={{ fontSize: 10 }}>→ {imp.resolved.split('/').slice(-2).join('/')}</span>}
        </div>
      ))}
    </>)}
    {dependents.length > 0 && (<>
      <div className="section-title">Dependents</div>
      {dependents.slice(0, 20).map((dep, i) => (
        <div key={i} className="file-item">
          <span className="path" style={{ fontSize: 12 }}>{dep.path}</span>
          <span className="lang" style={{ fontSize: 10 }}>{dep.spec}</span>
        </div>
      ))}
    </>)}
  </>)
}

/* ─── Impact ─── */
function ImpactPanel({ query, data }: { query: string; data: ImpactData | null }) {
  if (!query.trim()) return emptyState('🎯', 'Impact Analysis', 'Rank files by symbol change impact.', 'find', 'run', 'search')
  if (!data || !data.results?.length) return emptyResult('No impact', `No impact for "${query}"`)
  const maxScore = Math.max(...data.results.map(r => r.score), 1)
  return (<>
    <div className="result-count">Impact for &ldquo;{data.name}&rdquo; · {data.total} files</div>
    {data.results.map((r, i) => (
      <div key={i} className="file-item">
        <div className="impact-bar-wrap"><div className="impact-bar" style={{ width: `${(r.score / maxScore) * 100}%`, opacity: Math.max(0.3, r.score / maxScore) }}></div></div>
        <span className="path" style={{ fontSize: 12, flex: 1 }}>{r.path}</span>
        <span className="score" style={{ minWidth: 30, textAlign: 'right' }}>{r.score}</span>
        <span className="impact-reason">{r.reasons?.join(', ')}</span>
      </div>
    ))}
  </>)
}

/* ─── Map ─── */
function MapNodeDisplay({ node, depth, files }: { node: MapNode; depth: number; files: number }) {
  const pct = files > 0 ? ((node.file_count / files) * 100).toFixed(1) : '0'
  const hasChildren = node.children?.length > 0 && !node.truncated
  return (<>
    <div className="map-entry" style={{ paddingLeft: 12 + depth * 20 }}>
      <span className={node.is_dir ? 'map-icon-dir' : 'map-icon-file'}>{node.is_dir ? '📁' : '📄'}</span>
      <span className="map-name">{node.name}{node.truncated ? ' …' : ''}</span>
      {node.is_dir && <span className="map-meta">{node.file_count} files · {node.est_tokens > 1000 ? `${(node.est_tokens / 1000).toFixed(0)}k` : node.est_tokens} tok · {pct}%</span>}
      {!node.is_dir && node.symbols && node.symbols.length > 0 && <span className="map-meta" style={{ fontSize: 10 }}>• {node.symbols.slice(0, 3).map(s => s.name).join(', ')}</span>}
    </div>
    {hasChildren && node.children.map((c, i) => <MapNodeDisplay key={i} node={c} depth={depth + 1} files={files} />)}
  </>)
}

function MapPanel({ data }: { data: MapData | null }) {
  if (!data) return <div className="empty-state"><div className="big-icon">🗺</div><h3>Loading map…</h3></div>
  return (<>
    <div className="result-count">
      <span className="stat"><span className="num">{data.total_files}</span> files</span>
      <span className="stat"><span className="num">{(data.total_bytes / 1024 / 1024).toFixed(1)}MB</span></span>
      <span className="stat"><span className="num">{(data.total_est_tokens / 1000).toFixed(0)}k</span> tokens</span>
    </div>
    <MapNodeDisplay node={data.tree} depth={0} files={data.total_files} />
  </>)
}

/* ─── About ─── */
function AboutPanel({ overview }: { overview: OverviewData | null }) {
  return (
    <div className="about">
      <div className="about-hero">
        <span className="about-logo">⚡</span>
        <h2>FFS — Fast File Search</h2>
        <p className="about-tagline">A code-aware file search CLI for humans and AI agents. Really fast.</p>
      </div>
      {overview && (
        <div className="about-stats">
          <div className="about-stat"><span className="num">{overview.files}</span> files</div>
          <div className="about-stat"><span className="num">{(overview.code_lines / 1000).toFixed(1)}k</span> lines</div>
          <div className="about-stat"><span className="num">{overview.languages.length}</span> langs</div>
          <div className="about-stat"><span className="num">{overview.build_files}</span> build</div>
        </div>
      )}
      <h3>Commands</h3>
      <div className="cmd-grid">
        {[['find','Fuzzy file name search. Replaces find, fd.'],['glob','Glob pattern matching. Replaces glob, shell **.'],['grep','SIMD content search. Replaces grep, rg.'],['read','Token-budget aware file reader.'],['symbol','Tree-sitter symbol definitions.'],['callers','Find call sites of a symbol.'],['callees','Symbols referenced inside a definition.'],['refs','Definitions + usages in one shot.'],['flow','Drill-down: def + body + callees + callers.'],['siblings','Peer symbols in same parent scope.'],['deps','File imports + workspace dependents.'],['impact','Rank files by symbol change impact.'],['outline','Structural file outline.'],['dispatch','Auto-classify free-form queries.'],['map','Workspace tree with file count/tokens.'],['overview','High-signal workspace summary.'],['index','Build on-disk indexes.'],['mcp','MCP server over stdio (16 tools).']].map(([cmd, desc]) => (
          <div key={cmd} className="cmd-item"><code>{cmd}</code><span>{desc}</span></div>
        ))}
      </div>
      <h3>Architecture</h3>
      <pre className="arch">
ffs-cli     (binary)
ffs-mcp     (MCP server)
ffs-c       (C ABI library)
──────────────────────────
ffs-engine  (dispatch, ranking)
ffs-grep    (SIMD search)
ffs-symbol  (tree-sitter index)
ffs-budget  (token-aware reader)
ffs-query-parser  (DSL)
──────────────────────────
ffs-core    (scan, frecency, scoring)
      </pre>
      {overview?.top_symbols && overview.top_symbols.length > 0 && (
        <div style={{ marginTop: 16 }}>
          <h3>Top Symbols</h3>
          {overview.top_symbols.slice(0, 12).map((s, i) => (
            <div key={i} className="file-item">
              <span className="sym-kind" style={{ background: `${kindColor(s.kind)}22`, color: kindColor(s.kind), borderColor: `${kindColor(s.kind)}44` }}>{symIcon(s.kind)}</span>
              <span className="path">{s.name}</span>
              <span className="lang">{s.kind.replace('_item', '')}</span>
              <span className="score" style={{ fontSize: 11 }}>{s.weight}</span>
            </div>
          ))}
        </div>
      )}
      <p className="about-footer"><a href="https://github.com/quangdang46/fast_file_search" target="_blank">GitHub →</a></p>
    </div>
  )
}

/* ═══ Detail sub-panels ═══ */

function ReadDetail({ data }: { data: ReadData }) {
  return <pre className="detail-body">{data.body}</pre>
}

function OutlineDetail({ data }: { data: OutlineData }) {
  return (<div>{data.entries.map((e, i) => <OutlineEntryDisplay key={i} entry={e} depth={0} />)}</div>)
}

function RefsDetail({ data }: { data: RefsData }) {
  const defs = data.definitions || []; const uses = data.usages || []
  return (<>
    {defs.slice(0, 10).map((d, i) => (
      <div key={i} className="detail-sym-card">
        <div className="detail-sym-header">
          <span className="sym-kind" style={{ background: `${kindColor(d.kind)}22`, color: kindColor(d.kind), borderColor: `${kindColor(d.kind)}44` }}>{symIcon(d.kind)}</span>
          <span className="detail-sym-name">{d.path?.split('/').pop()}</span>
          <span className="detail-sym-loc">L{d.line}</span>
          <span className="lang" style={{ marginLeft: 'auto' }}>{d.kind.replace('_item', '')}</span>
        </div>
        {d.header && <pre className="detail-code">{d.header}</pre>}
      </div>
    ))}
    {uses.length > 0 && <div className="section-title">Usages ({data.total_usages})</div>}
    {uses.slice(0, 15).map((u, i) => (
      <div key={i} className="detail-match-line">
        <span className="detail-match-num">L{u.line}</span>
        <span className="detail-match-text">{u.text}</span>
      </div>
    ))}
  </>)
}

function FlowDetail({ data }: { data: FlowData }) {
  return (<>
    {data.cards.slice(0, 20).map((card, i) => (
      <div key={i} className="detail-flow-card">
        <div className="detail-flow-header">
          <span className="sym-kind" style={{ background: `${kindColor(card.def.kind)}22`, color: kindColor(card.def.kind), borderColor: `${kindColor(card.def.kind)}44`, width: 18, height: 18, fontSize: 9, display: 'inline-flex', marginRight: 6 }}>{symIcon(card.def.kind)}</span>
          {card.def.path?.split('/').pop()} L{card.def.line} · {card.def.kind.replace('_item', '')}
        </div>
        <pre className="detail-flow-body">{card.body?.slice(0, 400)}{card.body && card.body.length > 400 ? '…' : ''}</pre>
        <div className="detail-flow-footer">
          {card.callees && <span>{card.callees.length} callees</span>}
          {card.callers && <span>{card.callers.length} callers</span>}
        </div>
      </div>
    ))}
  </>)
}

function SiblingsDetail({ data }: { data: SiblingsData }) {
  const byFile: Record<string, SiblingHit[]> = {}
  for (const h of data.hits || []) { if (!byFile[h.path]) byFile[h.path] = []; byFile[h.path].push(h) }
  return (<>
    {Object.entries(byFile).slice(0, 10).map(([path, syms], i) => (
      <div key={i}>
        <div className="section-title">{path.split('/').pop()}</div>
        {syms.slice(0, 10).map((s, j) => (
          <div key={j} className="detail-sym-card" style={{ padding: '6px 16px' }}>
            <div className="detail-sym-header">
              <span className="sym-kind" style={{ background: `${kindColor(s.kind)}22`, color: kindColor(s.kind), borderColor: `${kindColor(s.kind)}44`, width: 18, height: 18, fontSize: 9 }}>{symIcon(s.kind)}</span>
              <span className="detail-sym-name" style={{ fontSize: 12 }}>{s.name}</span>
              <span className="detail-sym-loc">L{s.line}</span>
              <span className="lang" style={{ marginLeft: 'auto', fontSize: 9 }}>{s.kind}</span>
            </div>
          </div>
        ))}
      </div>
    ))}
  </>)
}

function ImpactDetail({ data }: { data: ImpactData }) {
  const maxScore = Math.max(...(data.results || []).map(r => r.score), 1)
  return (<>
    {data.results?.slice(0, 30).map((r, i) => (
      <div key={i} className="detail-impact-file">
        <div className="detail-impact-header">
          <div className="impact-bar-wrap" style={{ width: 60 }}><div className="impact-bar" style={{ width: `${(r.score / maxScore) * 100}%`, opacity: Math.max(0.3, r.score / maxScore) }}></div></div>
          <span className="detail-impact-score">{r.score}</span>
          <span className="detail-sym-name" style={{ fontSize: 12 }}>{r.path.split('/').pop()}</span>
        </div>
        <div className="detail-impact-reason">{r.reasons?.join(', ')}</div>
      </div>
    ))}
  </>)
}

function DepsDetail({ data }: { data: DepsData }) {
  return (<>
    <div className="section-title">Imports ({data.imports?.length || 0})</div>
    {(data.imports || []).map((imp, i) => (
      <div key={i} className="detail-match-line">
        <span className="detail-match-num" style={{ minWidth: 24 }}>{i + 1}</span>
        <span className="detail-match-text">{imp.spec}</span>
        {imp.resolved && <span className="lang" style={{ marginLeft: 'auto', fontSize: 9 }}>{imp.resolved.split('/').pop()}</span>}
      </div>
    ))}
    <div className="section-title" style={{ marginTop: 12 }}>Dependents ({data.total_dependents})</div>
    {(data.dependents || []).slice(0, 20).map((dep, i) => (
      <div key={i} className="detail-match-line">
        <span className="detail-match-text">{dep.path}</span>
        <span className="lang" style={{ marginLeft: 'auto', fontSize: 9 }}>{dep.spec}</span>
      </div>
    ))}
  </>)
}

/* ─── Helpers ─── */
function emptyState(icon: string, title: string, desc: string, ...hints: string[]) {
  return (
    <div className="empty-state">
      <div className="big-icon">{icon}</div>
      <h3>{title}</h3>
      <p>{desc}</p>
      {hints.length > 0 && <p className="hint">Try: {hints.map((h, i) => <span key={h}><kbd>{h}</kbd>{i < hints.length - 1 ? ' ' : ''}</span>)}</p>}
    </div>
  )
}

function emptyResult(title: string, desc: string) {
  return <div className="empty-state"><div className="big-icon" style={{ fontSize: 28 }}>—</div><h3>{title}</h3><p>{desc}</p></div>
}
