#!/usr/bin/env python3
"""Recompute the live metrics cited by CLOSING-review.md.

The memory and call-graph measurements read SQLite in read-only mode. The
daemon stderr measurement necessarily inspects the running process: discarded
stderr bytes are an open-file offset and are not stored in either database.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
from pathlib import Path


CALLABLE_KINDS = ("Function", "Method", "Struct", "Class", "Enum", "Actor")

RESOLUTION_TIER = r"""
CASE
  WHEN r.to_qualifier IS NOT NULL THEN CASE
    WHEN s.owner_name IS NOT NULL AND (
      r.to_qualifier = s.owner_name
      OR substr(r.to_qualifier, -length(s.owner_name) - 2) = '::' || s.owner_name
      OR substr(r.to_qualifier, -length(s.owner_name) - 1) = '.' || s.owner_name
      OR substr(r.to_qualifier, -length(s.owner_name) - 1) = '\' || s.owner_name
    ) THEN 1
    WHEN s.module_path IS NOT NULL AND (
      s.module_path = r.to_qualifier
      OR ((
        substr(s.module_path, -length(r.to_qualifier) - 2) = '::' || r.to_qualifier
        OR substr(s.module_path, -length(r.to_qualifier) - 1) = '.' || r.to_qualifier
      ) AND EXISTS (
        SELECT 1 FROM code_imports i WHERE i.file_id = r.file_id AND (
          i.path = s.module_path
          OR i.path = s.module_path || '::' || s.name
          OR i.path = s.module_path || '.' || s.name
        )
      ))
    ) THEN 2
    ELSE 3 END
  WHEN r.to_receiver_type IS NOT NULL THEN CASE
    WHEN s.owner_name IS NOT NULL AND (
      s.owner_name = r.to_receiver_type
      OR substr(s.owner_name, -length(r.to_receiver_type) - 2) = '::' || r.to_receiver_type
      OR substr(s.owner_name, -length(r.to_receiver_type) - 1) = '.' || r.to_receiver_type
      OR substr(s.owner_name, -length(r.to_receiver_type) - 1) = '\' || r.to_receiver_type
    ) THEN CASE WHEN s.file_id = r.file_id THEN 1 ELSE 2 END
    ELSE 3 END
  WHEN r.to_receiver_call IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM code_symbols o WHERE o.name = r.to_receiver_call
  ) THEN 3
  WHEN s.file_id = r.file_id THEN 4
  WHEN s.module_path IS NOT NULL AND EXISTS (
    SELECT 1 FROM code_imports i WHERE i.file_id = r.file_id AND (
      i.path = s.module_path
      OR i.path = s.module_path || '::' || s.name
      OR i.path = s.module_path || '.' || s.name
    )
  ) THEN 5
  WHEN s.module_path IS NOT NULL AND s.module_path = fs.module_path THEN 6
  ELSE 7
END
"""


def readonly(path: Path) -> sqlite3.Connection:
    if not path.is_file():
        raise SystemExit(f"missing database: {path}")
    return sqlite3.connect(f"file:{path.resolve()}?mode=ro", uri=True)


def memory_gate(index: sqlite3.Connection, threshold: float) -> dict[str, object]:
    rows = index.execute(
        """
        SELECT id, confirmations, source_type
        FROM memory_entries
        WHERE status = 'active' AND entry_type IN ('topic', 'problem', 'decision')
        """
    ).fetchall()
    authority = {
        "official_docs": 1.0,
        "user_statement": 0.85,
        "auto_extracted": 0.70,
        "inference": 0.65,
    }
    scores = []
    for entry_id, confirmations, source_type in rows:
        belief = (1.0 + confirmations) / (2.0 + confirmations)
        score = max(belief * authority.get(source_type, 0.85), 0.05)
        scores.append((entry_id, score))
    return {
        "clearing": sum(score >= threshold for _, score in scores),
        "total": len(scores),
        "threshold": threshold,
    }


def resolved_cte() -> str:
    kinds = ",".join(f"'{kind}'" for kind in CALLABLE_KINDS)
    return f"""
    WITH candidates AS (
      SELECT r.id AS edge_id, r.from_symbol_id AS from_id, s.id AS sym_id,
             s.kind AS target_kind, {RESOLUTION_TIER} AS tier,
             MIN({RESOLUTION_TIER}) OVER (PARTITION BY r.id) AS nearest
      FROM code_relationships r
      JOIN code_symbols fs ON fs.id = r.from_symbol_id
      JOIN code_symbols s ON s.name = r.to_name AND s.kind IN ({kinds})
      WHERE r.kind = 'Calls' AND r.from_symbol_id IS NOT NULL
    ), resolved AS (
      SELECT * FROM candidates WHERE tier = nearest AND nearest <> 3
    )
    """


def code_metrics(code: sqlite3.Connection) -> tuple[dict[str, object], dict[str, object]]:
    cte = resolved_cte()
    edges, slots, noncallable = code.execute(
        cte
        + """
        SELECT COUNT(DISTINCT edge_id), COUNT(*),
               SUM(target_kind NOT IN ('Function','Method','Struct','Class','Enum','Actor'))
        FROM resolved
        """
    ).fetchone()
    suppressed = code.execute(
        cte
        + """
        SELECT COUNT(*) FROM (
          SELECT DISTINCT
            CASE WHEN ff.rel_path < tf.rel_path THEN ff.rel_path ELSE tf.rel_path END AS file_a,
            CASE WHEN ff.rel_path < tf.rel_path THEN tf.rel_path ELSE ff.rel_path END AS file_b
          FROM resolved e
          JOIN code_symbols fs ON fs.id = e.from_id
          JOIN code_files ff ON ff.id = fs.file_id
          JOIN code_symbols ts ON ts.id = e.sym_id
          JOIN code_files tf ON tf.id = ts.file_id
          WHERE e.nearest <= 2 AND ff.rel_path <> tf.rel_path
        )
        """
    ).fetchone()[0]
    files = code.execute("SELECT COUNT(*) FROM code_files").fetchone()[0]
    possible_pairs = files * (files - 1) // 2
    return (
        {
            "resolving_edges": edges,
            "candidate_slots": slots,
            "non_callable_slots": noncallable or 0,
        },
        {
            "suppressed_pairs": suppressed,
            "possible_unordered_pairs": possible_pairs,
            "indexed_files": files,
        },
    )


def daemon_stderr() -> dict[str, object]:
    try:
        processes = subprocess.run(
            ["pgrep", "-f", "mdkb serve --daemon"],
            check=False,
            capture_output=True,
            text=True,
        ).stdout.split()
    except FileNotFoundError:
        return {"status": "unavailable", "reason": "pgrep is not installed"}
    if not processes:
        return {"status": "not_running", "lost_bytes": 0}
    pid = processes[0]
    try:
        output = subprocess.run(
            ["lsof", "-a", "-p", pid, "-d", "2", "-Fn", "-Fo"],
            check=False,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
    except FileNotFoundError:
        return {"status": "unavailable", "pid": int(pid), "reason": "lsof is not installed"}
    name = next((line[1:] for line in output if line.startswith("n")), None)
    offset = next((line[3:] for line in output if line.startswith("o0t")), "0")
    lost = int(offset) if name == "/dev/null" and offset.isdigit() else 0
    return {"status": "measured", "pid": int(pid), "target": name, "lost_bytes": lost}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--min-recall-score", type=float, default=0.3)
    args = parser.parse_args()
    store = args.root / ".mdkb"
    with readonly(store / "index.sqlite") as index, readonly(store / "code.sqlite") as code:
        calls, coupling = code_metrics(code)
        result = {
            "root": str(args.root.resolve()),
            "memory_recall_gate": memory_gate(index, args.min_recall_score),
            "call_resolution": calls,
            "coupling": coupling,
            "daemon_stderr": daemon_stderr(),
        }
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
