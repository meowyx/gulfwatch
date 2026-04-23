"use client";

import { useEffect, useState } from "react";
import { Glyph } from "./glyph";

const PROGS = [
  { id: "Toke..xuEb", label: "Token Program", base: 33 },
  { id: "675k..1Mp8", label: "System", base: 7 },
  { id: "CAMM..rWqK", label: "Raydium CLMM", base: 42 },
  { id: "JUP6..TaV4", label: "Jupiter", base: 83 },
  { id: "METE..oraX", label: "Meteora", base: 19 },
] as const;

const TYPES = [
  "transfer",
  "fee-only",
  "swap",
  "transferCh",
  "initMint",
  "closeAcct",
  "failed-tra",
  "mintTo",
] as const;

const HEX = "abcdef0123456789";

type Row = {
  id: number;
  sig: string;
  prog: string;
  type: string;
  cu: string;
  err: boolean;
};

// Fixed seed rows — must be deterministic for SSR to match first client render.
const SEED_ROWS: Row[] = [
  { id: 1, sig: "9f3a2b10d1..", prog: "JUP6..TaV4", type: "swap", cu: "142.3K", err: false },
  { id: 2, sig: "3c81e074aa..", prog: "Toke..xuEb", type: "transfer", cu: "38.7K", err: false },
  { id: 3, sig: "bd42e71f06..", prog: "CAMM..rWqK", type: "swap", cu: "198.1K", err: false },
  { id: 4, sig: "710aac9e22..", prog: "JUP6..TaV4", type: "failed-tra", cu: "210.6K", err: true },
  { id: 5, sig: "05f8b13c4e..", prog: "675k..1Mp8", type: "fee-only", cu: "5.0K", err: false },
  { id: 6, sig: "ee239ab801..", prog: "Toke..xuEb", type: "mintTo", cu: "51.2K", err: false },
  { id: 7, sig: "4a6c7f30d9..", prog: "METE..oraX", type: "transferCh", cu: "87.4K", err: false },
  { id: 8, sig: "18e705ba55..", prog: "JUP6..TaV4", type: "swap", cu: "161.8K", err: false },
];

function randSig() {
  let s = "";
  for (let i = 0; i < 10; i++) s += HEX[(Math.random() * 16) | 0];
  return s + "..";
}

let rowCounter = SEED_ROWS.length + 1;
function randomRow(errBias = 0.15): Row {
  const p = PROGS[(Math.random() * PROGS.length) | 0];
  const t = TYPES[(Math.random() * TYPES.length) | 0];
  const err = t === "failed-tra" || Math.random() < errBias;
  return {
    id: rowCounter++,
    sig: randSig(),
    prog: p.id,
    type: err ? "failed-tra" : t,
    cu: (30 + Math.random() * 220).toFixed(1) + "K",
    err,
  };
}

function seedCounts(): Record<string, number> {
  const c: Record<string, number> = {};
  PROGS.forEach((p) => (c[p.id] = p.base));
  return c;
}

type Props = {
  compact?: boolean;
};

export function LiveTerminal({ compact = false }: Props) {
  const [rows, setRows] = useState<Row[]>(SEED_ROWS);
  const [counts, setCounts] = useState<Record<string, number>>(seedCounts);
  const [errs, setErrs] = useState(28);
  const [total, setTotal] = useState(165);

  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    if (media.matches) return;

    const iv = setInterval(() => {
      const row = randomRow();
      setRows((prev) => [row, ...prev].slice(0, 9));
      setCounts((prev) => ({ ...prev, [row.prog]: (prev[row.prog] || 0) + 1 }));
      setTotal((prev) => prev + 1);
      if (row.err) setErrs((prev) => prev + 1);
    }, 900);

    return () => clearInterval(iv);
  }, []);

  return (
    <div
      aria-label="Sample transaction stream, animated"
      className="bg-dark-bg border border-dark-border rounded-[4px] overflow-hidden font-mono text-dark-fg"
      style={{
        boxShadow:
          "0 30px 80px -20px rgba(0,0,0,0.5), 0 0 0 1px rgba(255,255,255,0.02) inset",
      }}
    >
      {/* Title bar */}
      <div className="flex items-center gap-2.5 px-3.5 py-2.5 border-b border-dark-border text-xs">
        <span className="flex gap-1.5">
          <span className="w-[11px] h-[11px] bg-[#3a3e40] rounded-full" />
          <span className="w-[11px] h-[11px] bg-[#3a3e40] rounded-full" />
          <span className="w-[11px] h-[11px] bg-[#3a3e40] rounded-full" />
        </span>
        <Glyph size={16} />
        <span className="tracking-[0.1em] font-bold text-dark-fg">
          GULFWATCH
        </span>
        <span className="text-dark-label">│</span>
        <span className="text-[#7a8287]">Solana Program Observability</span>
        <span className="ml-auto text-[11px] text-dark-label">
          <span className="text-primary-dark">●</span> streaming · {total} tx ·{" "}
          <span className="text-accent-dark">{errs} err</span>
        </span>
      </div>

      {/* Body */}
      <div
        className="flex"
        style={{ height: compact ? 340 : 440 }}
      >
        {/* Sidebar */}
        <div className="w-[190px] border-r border-dark-border px-3.5 py-3 text-xs overflow-hidden">
          <div className="text-dark-label text-[10px] tracking-[0.15em] mb-2.5">
            PROGRAMS [{PROGS.length}]
          </div>
          <div className="flex justify-between text-primary-dark mb-2">
            <span>▶ All</span>
            <span>{total}</span>
          </div>
          {PROGS.map((p) => {
            const isJup = p.id === "JUP6..TaV4";
            return (
              <div key={p.id} className="mb-2">
                <div className="flex justify-between text-dark-fg">
                  <span>{p.id}</span>
                  <span className="tabular-nums">{counts[p.id]}</span>
                </div>
                {isJup && (
                  <div className="text-[#e25d5d] text-[11px] mt-0.5">
                    {"  "}
                    {errs} err
                  </div>
                )}
              </div>
            );
          })}
        </div>

        {/* Table */}
        <div className="flex-1 px-3.5 py-3 text-xs">
          <div
            className="grid gap-2.5 text-dark-label text-[10px] tracking-[0.15em] mb-2.5"
            style={{ gridTemplateColumns: "1.2fr 18px 1fr 1fr 80px" }}
          >
            <span>SIG</span>
            <span />
            <span>PROGRAM</span>
            <span>TYPE</span>
            <span className="text-right">CU</span>
          </div>
          <div className="flex flex-col gap-1.5">
            {rows.map((r, i) => (
              <div
                key={r.id}
                className="grid gap-2.5 tabular-nums"
                style={{
                  gridTemplateColumns: "1.2fr 18px 1fr 1fr 80px",
                  color: r.err ? "#e25d5d" : undefined,
                  opacity: i === 0 ? 0 : 1 - i * 0.02,
                  animation:
                    i === 0 ? "gwFadeIn 0.4s ease forwards" : undefined,
                }}
              >
                <span>{r.sig}</span>
                <span
                  style={{ color: r.err ? "#e25d5d" : "#b8e28a" }}
                >
                  {r.err ? "✗" : "✓"}
                </span>
                <span>{r.prog}</span>
                <span>{r.type}</span>
                <span className="text-right">{r.cu}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
