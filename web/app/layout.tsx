import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "GulfWatch - a runtime intelligence for Solana",
  description:
    "Terminal-first observability for Solana. One ingest feeds a keyboard-driven TUI and an MCP server; humans and agents see the same truth.",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" className="h-full antialiased">
      <body className="min-h-full flex flex-col">{children}</body>
    </html>
  );
}
