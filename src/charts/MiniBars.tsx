/** 表格行内迷你近况柱（zpool 式；round-02 原型未挂载，保留组件供接入真数据时使用） */
export function MiniBars({ values, w = 64, h = 18 }: { values: readonly number[]; w?: number; h?: number }) {
  const max = Math.max(...values, 1);
  return (
    <span style={{ display: "inline-flex", alignItems: "flex-end", gap: 2, width: w, height: h }} aria-hidden>
      {values.map((v, i) => (
        <i
          key={i}
          style={{
            flex: 1,
            height: Math.max(8, (v / max) * 100) + "%",
            borderRadius: 1,
            background: v >= max ? "var(--blue)" : "rgba(59,130,246,.45)",
          }}
        />
      ))}
    </span>
  );
}
