#!/bin/bash
# Extract FFS demo data — all features
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/demo/public/data"
rm -rf "$OUT" && mkdir -p "$OUT"

echo "=== File listing ==="
ffs glob "**" --format json --root "$ROOT" 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
root = '$ROOT'
matches = []
for m in d['matches']:
    if m.startswith('demo/'): continue
    if m.startswith(root): m = m[len(root)+1:]
    matches.append(m)
d['matches'] = sorted(set(matches))
json.dump(d, sys.stdout, separators=(',',':'))
" > "$OUT/files.json"

echo "=== Overview ==="
ffs overview --format json --root "$ROOT" > "$OUT/overview.json" 2>/dev/null

echo "=== Symbols ==="
for sym in "run" "new" "default" "find" "search" "read" "parse" "build" "format"; do
  ffs symbol "$sym" --format json --root "$ROOT" 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
root = '$ROOT'
items = []
for h in d.get('hits', []):
    p = h.get('path','')
    if p.startswith(root): p = p[len(root)+1:]
    items.append({'n': h['name'], 'k': h['kind'], 'f': p, 'l': h.get('line',0)})
print(json.dumps({'q': '$sym', 'hits': items}))
" > "$OUT/sym_${sym}.json"
done

echo "=== Callers ==="
for sym in "run" "new" "find" "search" "fuzzy_search" "dispatch"; do
  ffs callers "$sym" --format json --root "$ROOT" 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
root = '$ROOT'
items = []
for h in d.get('hits', []):
    p = h.get('path','')
    if p.startswith(root): p = p[len(root)+1:]
    items.append({'f': p, 'l': h.get('line',0), 'txt': h.get('text','')[:120], 'target': h.get('target',''), 'd': h.get('depth',1)})
print(json.dumps({'q': '$sym', 'hits': items, 'total': len(items)}))
" > "$OUT/callers_${sym}.json"
done

echo "=== Callees ==="
for sym in "run" "new" "fuzzy_search" "dispatch"; do
  ffs callees "$sym" --format json --root "$ROOT" 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
root = '$ROOT'
items = []
for h in d.get('hits', []):
    p = h.get('path','')
    if p.startswith(root): p = p[len(root)+1:]
    items.append({'n': h.get('name',''), 'f': p, 'l': h.get('line',0)})
print(json.dumps({'q': '$sym', 'hits': items, 'total': len(items)}))
" > "$OUT/callees_${sym}.json"
done

echo "=== Grep samples ==="
for pattern in "fn " "struct " "RwLock" "unsafe" "TODO" "fuzzy_file_search"; do
  sfx=$(echo "$pattern" | tr ' ' '_')
  ffs grep "$pattern" --format json 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
root = '$ROOT'
items = []
for h in d.get('hits', []):
    p = h.get('path','')
    if p.startswith(root): p = p[len(root)+1:]
    items.append({'f': p, 'l': h.get('line',0), 'txt': h.get('text','')[:150]})
# Group by file, max 10 files
seen = {}
grouped = []
for i in items:
    if i['f'] not in seen and len(seen) < 10:
        seen[i['f']] = True
        grouped.append(i)
print(json.dumps({'q': '$pattern', 'matches': grouped, 'total': len(items)}))
" > "$OUT/grep_${sfx}.json"
done

echo "=== Outlines (key files) ==="
for file in \
  "crates/ffs-core/src/fuzzy_file_search.rs" \
  "crates/ffs-core/src/file_picker.rs" \
  "crates/ffs-cli/src/cli.rs" \
  "crates/ffs-engine/src/dispatch.rs" \
  "crates/ffs-symbol/src/symbol_index.rs"; do
  ffs outline "$file" --format json --root "$ROOT" 2>/dev/null > "$OUT/ol_$(echo $file | tr / _).json" || true
done

echo "=== Done ==="
du -sh "$OUT"/
