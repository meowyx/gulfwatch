"use client";

import { useState } from "react";

const COMMAND = "cargo install gulfwatch";

export function InstallCommand() {
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(COMMAND);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Silent: older browsers without clipboard API.
    }
  }

  return (
    <button
      type="button"
      onClick={handleCopy}
      aria-label={`Copy install command: ${COMMAND}`}
      className={`px-5 py-3.5 bg-primary-dark text-dark-bg tracking-[0.04em] font-bold font-mono text-[13px] cursor-pointer transition-all duration-200 hover:bg-[#c7ec99] hover:-translate-y-0.5 ${
        copied ? "" : "cursor-blink"
      }`}
    >
      {copied ? "copied!" : `$ ${COMMAND}`}
    </button>
  );
}
