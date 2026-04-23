import { FadeInUp } from "./components/fade-in-up";
import { Glyph } from "./components/glyph";
import { InstallCommand } from "./components/install-command";
import { LiveTerminal } from "./components/live-terminal";

const LADDER: Array<[string, string, string, string]> = [
  ["01", "observe", "Every signature, streamed live. No polling.", "live stream"],
  [
    "02",
    "inspect",
    "Drill into accounts, instructions, logs, CU.",
    "8-tab deep-dive",
  ],
  [
    "03",
    "understand",
    "Grouped errors, heatmaps, throughput per program.",
    "8 detection rules",
  ],
  [
    "04",
    "act",
    "Pipe to MCP agents, Slack, or scripts.",
    "MCP + webhooks",
  ],
];

export default function Home() {
  return (
    <div className="min-h-full bg-cream text-cream-fg">
      {/* ── Dark hero ───────────────────────────────────────── */}
      <section className="bg-dark-bg text-dark-fg pb-20">
        <nav className="flex items-center gap-5 px-12 py-5 border-b border-dark-border font-mono text-xs">
          <Glyph size={18} />
          <span className="font-bold tracking-[0.14em]">GULFWATCH</span>
          <span className="ml-auto flex gap-6 tracking-[0.15em] text-dark-muted">
            <a
              href="https://github.com/meowyx/gulfwatch/tree/main/docs"
              className="transition-colors hover:text-dark-fg"
            >
              docs
            </a>
            <a
              href="https://github.com/meowyx/gulfwatch/blob/main/README.md"
              className="transition-colors hover:text-dark-fg"
            >
              guides
            </a>
            <a
              href="https://github.com/meowyx/gulfwatch"
              className="transition-colors hover:text-dark-fg"
            >
              github ↗
            </a>
          </span>
        </nav>

        <div className="grid grid-cols-[1.3fr_1fr] gap-14 items-end px-12 pt-24 pb-12">
          <div className="hero-fade hero-fade-delay-1">
            <div className="font-mono text-[10px] tracking-[0.35em] text-dark-muted mb-6">
              <span className="text-primary-dark">●</span> pre-alpha · runtime
              observability · solana
            </div>
            <h1 className="font-display text-[96px] font-normal leading-[0.96] tracking-[-0.012em] text-dark-fg text-balance m-0">
              the runtime your{" "}
              <em className="text-primary-dark italic">programs</em> deserve.
            </h1>
          </div>
          <p className="hero-fade hero-fade-delay-2 font-display text-lg text-[#a8adb0] leading-[1.5] italic max-w-[380px] m-0">
            Terminal-first observability for Solana. One ingest feeds a
            keyboard-driven TUI <em>and</em> an MCP server; humans and agents
            see the same truth.
          </p>
        </div>

        <div className="hero-fade hero-fade-delay-3 px-12 flex gap-3.5 items-center font-mono text-[13px]">
          <InstallCommand />
          <a
            href="https://github.com/meowyx/gulfwatch"
            className="px-5 py-3.5 border border-dark-border text-dark-fg tracking-[0.1em] transition-all duration-200 hover:bg-dark-surface hover:border-dark-muted hover:-translate-y-0.5"
          >
            ★ view on github ↗
          </a>
        </div>

        {/* Feature ladder */}
        <div className="px-12 pt-24 pb-4 font-mono">
          <div className="text-dark-label text-[10px] tracking-[0.25em] mb-6">
            // features
          </div>
          {LADDER.map(([n, title, desc, stat], i) => (
            <FadeInUp
              key={n}
              delay={i * 80}
              className="grid items-baseline gap-8 py-6 border-t border-dark-border"
            >
              <div
                className="grid items-baseline gap-8"
                style={{ gridTemplateColumns: "60px 1fr 2fr 200px" }}
              >
                <span className="text-primary-dark text-[13px]">{n}.</span>
                <span className="text-[22px] tracking-[0.14em] font-bold text-dark-fg">
                  {title}
                </span>
                <span className="text-sm text-dark-muted">{desc}</span>
                <span className="text-[13px] text-accent-dark text-right">
                  {stat}
                </span>
              </div>
            </FadeInUp>
          ))}
          <div className="border-t border-dark-border" />
        </div>
      </section>

      {/* ── Terminal section ────────────────────────────────── */}
      <FadeInUp className="px-14 py-20">
        <div className="flex items-baseline justify-between mb-7">
          <div>
            <div className="font-mono text-[10px] tracking-[0.35em] text-cream-label mb-2">
              § sample view
            </div>
            <h2 className="m-0 text-[52px] font-normal tracking-[-0.01em]">
              a glimpse inside the TUI.
            </h2>
          </div>
          <p className="italic text-cream-muted text-base max-w-[360px] m-0">
            Rows stream in as transactions finalize. Reds are failures. Sidebar
            counts climb per program.
          </p>
        </div>
        <LiveTerminal />
      </FadeInUp>

      {/* ── Footer ──────────────────────────────────────────── */}
      <footer className="px-14 py-9 border-t border-cream-border flex items-center gap-5 font-mono text-[11px] text-cream-label tracking-[0.1em]">
        <Glyph size={18} primary="#2d5a78" accent="#b8562a" />
        <span>© 2026 · Gulfwatch</span>
        <span className="ml-auto flex gap-6">
          <a
            href="https://github.com/meowyx/gulfwatch"
            className="transition-colors hover:text-cream-fg"
          >
            github
          </a>
          <a
            href="https://github.com/meowyx/gulfwatch/tree/main/docs"
            className="transition-colors hover:text-cream-fg"
          >
            docs
          </a>
          <a
            href="https://x.com/me256ow"
            className="transition-colors hover:text-cream-fg"
          >
            x
          </a>
        </span>
      </footer>
    </div>
  );
}
