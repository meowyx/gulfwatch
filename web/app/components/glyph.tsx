const GRID = [
  ".XXXXXX.",
  "XX....XX",
  "XX......",
  "XX..XXXX",
  "XX....XX",
  "XX...oXX",
  "XX....XX",
  ".XXXXXX.",
];

type GlyphProps = {
  size?: number;
  primary?: string;
  accent?: string;
  pad?: number;
};

export function Glyph({
  size = 48,
  primary = "#b8e28a",
  accent = "#e8b75a",
  pad = 1,
}: GlyphProps) {
  const total = GRID.length + pad * 2;
  const cells: Array<{ x: number; y: number; fill: string }> = [];
  GRID.forEach((row, y) => {
    [...row].forEach((c, x) => {
      if (c === "X") cells.push({ x: x + pad, y: y + pad, fill: primary });
      else if (c === "o")
        cells.push({ x: x + pad, y: y + pad, fill: accent });
    });
  });

  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${total} ${total}`}
      shapeRendering="crispEdges"
      style={{ display: "block" }}
      aria-hidden="true"
    >
      {cells.map((c, i) => (
        <rect key={i} x={c.x} y={c.y} width="1" height="1" fill={c.fill} />
      ))}
    </svg>
  );
}
