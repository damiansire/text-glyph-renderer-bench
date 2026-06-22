# Duel A — Data Structures: Rope vs. Piece Table

---

## Rope (Tree of Text Nodes)

```
        [0..200]
       /         \
  [0..100]    [101..200]
  /    \        /    \
[0..50][51..100][101..150][151..200]
  "Hello..."  "World..."  ...
```

| Operation | Complexity | Condition |
|-----------|------------|-----------|
| Insert/Delete | O(log N) | N = total bytes |
| Slice (access) | O(log N) | with concatenation |
| Line-at-byte | O(log N) | if nodes store `\n` count |
| Rebalancing | O(log N) amortized | after K mutations |

**Strength:** Insertions/deletions at any position are uniformly fast. Essential for collaborative editors (CRDT / OT).

**Weakness:** For this benchmark's use case — **100 MB immutable file with scroll** — node overhead (pointers, metadata, heap allocations) can exceed the cost of a simple mmap slice. Rebalancing can also cause CPU cache misses.

---

## Piece Table / Piece Tree

```
Original Buffer (mmap, read-only):
  [  100MB immutable file in physical RAM  ]

Add Buffer (in RAM, writable):
  [ accumulated insertions ]

Piece Table:
  Piece { buffer: Original, start: 0,      len: 5000 }
  Piece { buffer: Add,      start: 0,      len: 3    }   ← INSERT "abc"
  Piece { buffer: Original, start: 5000,   len: 95000 }
  ...
```

| Operation | Complexity | Condition |
|-----------|------------|-----------|
| Insert (append-only to Add buffer) | O(P + L) | O(1) piece split; line-index patch is O(L) lines |
| Delete | O(P + L) | piece split (no data movement); line-index patch is O(L) lines |
| Slice | O(P) over piece table | flat `Vec<Piece>`; P = number of pieces |
| Serialization | O(N_pieces × avg_size) | recombine pieces |

> **Note on complexity.** The piece-split itself is O(1) (Insert) / O(1) (Delete),
> but every edit also patches the line-index in place (`record_insert` /
> `record_delete`), which shifts/splices offsets and is O(L) in the number of
> lines — hence the O(P + L) above. Likewise, `Slice` is O(P) because the
> implemented buffer is a **flat `Vec<Piece>`** scanned linearly (`find_piece`,
> `slice_pieces`); the O(log N) cited for a *Piece Tree* (piece table backed by a
> search tree over line offsets) is the recommended design, **not** what is built
> here. The **line** index alone is O(log L) (`LineIndex::byte_to_line` uses
> `binary_search`).

**Critical strength for this benchmark:**
- The original buffer **is the mmap directly**. `Piece.start` + `Piece.len` index mapped memory without any `memcpy`. This is **CPU-side** zero-copy text access.
- For a read/scroll file, there are **zero mutations to the Add buffer** → the piece table has exactly 1 entry.
- Where Metal's `makeBuffer(bytesNoCopy:)` over the mmap helps, it makes the **raw UTF-8 bytes** available to the GPU without a copy. It does **not** mean "the rendered glyph comes from the same disk page": the rendered glyph is produced from a **CPU-rasterized atlas + a CPU-built vertex buffer**, not from the raw bytes. (And note: no PoC in this repo actually implements `bytesNoCopy` yet — this is the design target, not measured behaviour.)

---

## Verdict

For this benchmark's use case (**massive text + scroll + few edits**):

> **Piece Table wins** on access latency (O(1) slice for the no-mutation case) and memory usage (no node overhead). Rope wins in intensive collaborative editing scenarios, which is not the target profile.

Recommended implementation: a **Piece Tree** (Piece Table with a search tree for line offsets), like VS Code uses internally, but optimized so the original buffer IS the mmap pointer.
