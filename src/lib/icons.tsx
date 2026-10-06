/** lucide 风格 1.5px 描边图标（移植自 round-02 原型 app.js 的 P 表） */

const P: Record<string, string> = {
  search: '<circle cx="11" cy="11" r="7"/><path d="m20.5 20.5-4.2-4.2"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  refresh:
    '<path d="M3 12a9 9 0 0 1 15-6.7L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-15 6.7L3 16"/><path d="M3 21v-5h5"/>',
  download: '<path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/>',
  upload: '<path d="M12 21V9"/><path d="m7 14 5-5 5 5"/><path d="M5 3h14"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 8h.01M12 12v4"/>',
  power: '<path d="M18.4 6.6a9 9 0 1 1-12.8 0"/><path d="M12 2v8"/>',
  trash: '<path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="m19 6-1 14H6L5 6"/><path d="M10 11v6M14 11v6"/>',
  check: '<path d="m4 12 5 5L20 7"/>',
  okc: '<circle cx="12" cy="12" r="9"/><path d="m8.5 12.5 2.5 2.5 5-5.5"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  copy: '<rect x="9" y="9" width="12" height="12" rx="2.2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/>',
  eye: '<path d="M2 12s3.5-6.5 10-6.5S22 12 22 12s-3.5 6.5-10 6.5S2 12 2 12z"/><circle cx="12" cy="12" r="2.6"/>',
  eyeoff:
    '<path d="M3 3l18 18"/><path d="M10.6 5.1A10 10 0 0 1 22 12a15 15 0 0 1-2.7 3.3M6.6 6.6A15 15 0 0 0 2 12s3.5 6.5 10 6.5a9.7 9.7 0 0 0 4.3-1"/><path d="M9.9 9.9a2.6 2.6 0 0 0 3.7 3.7"/>',
  key: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m11 12 9-9"/><path d="M17 5l3 3"/>',
  zap: '<path d="M13 2 3 14h7l-1 8 11-14h-8l1-6z"/>',
  activity: '<path d="M22 12h-4l-3 8-6-16-3 8H2"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  warn: '<path d="M12 3 2 20h20L12 3z"/><path d="M12 9.5v4.5M12 17.5h.01"/>',
  alertr: '<circle cx="12" cy="12" r="9"/><path d="M12 8v5M12 16.5h.01"/>',
  gift:
    '<path d="M20 12v9H4v-9"/><rect x="2" y="7" width="20" height="5" rx="1"/><path d="M12 21V7"/><path d="M12 7H7.5a2.5 2.5 0 1 1 0-5C11 2 12 7 12 7z"/><path d="M12 7h4.5a2.5 2.5 0 1 0 0-5C13 2 12 7 12 7z"/>',
  cal: '<rect x="3" y="4" width="18" height="17" rx="2"/><path d="M16 2v4M8 2v4M3 10h18"/>',
  chevR: '<path d="m9 6 6 6-6 6"/>',
  chevL: '<path d="m15 6-6 6 6 6"/>',
  grid:
    '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  gear: '<path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h13M21 18h-1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="19" cy="18" r="2"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18z"/>',
  book:
    '<path d="M2 4h6a4 4 0 0 1 4 4v13a3 3 0 0 0-3-3H2z"/><path d="M22 4h-6a4 4 0 0 0-4 4v13a3 3 0 0 1 3-3h7z"/>',
  term: '<path d="m4 17 6-5-6-5"/><path d="M12 19h8"/>',
  users:
    '<path d="M15.6 20v-1.6a3.8 3.8 0 0 0-3.8-3.8H6.4A3.8 3.8 0 0 0 2.6 18.4V20"/><circle cx="9.1" cy="7.4" r="3.4"/><path d="M16.6 4.3a3.4 3.4 0 0 1 0 6.4M21.4 20v-1.6a3.8 3.8 0 0 0-2.8-3.7"/>',
  inbox:
    '<path d="M21.5 12h-4.6l-1.6 2.8H8.7L7.1 12H2.5"/><path d="M5.6 5.2h12.8l3.1 6.3v5.3a2 2 0 0 1-2 2H4.5a2 2 0 0 1-2-2v-5.3z"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  server:
    '<rect x="2" y="3" width="20" height="7" rx="2"/><rect x="2" y="14" width="20" height="7" rx="2"/><path d="M6 6.5h.01M6 17.5h.01"/>',
  shield: '<path d="M12 2l8 3.5V12c0 5-3.4 8.4-8 10-4.6-1.6-8-5-8-10V5.5z"/>',
  arrowR: '<path d="M5 12h14"/><path d="m13 6 6 6-6 6"/>',
  ext: '<path d="M14 3h7v7"/><path d="M21 3 11 13"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  play: '<path d="m6 4 14 8-14 8z"/>',
  pause: '<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>',
  login: '<path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/><path d="m10 17 5-5-5-5"/><path d="M15 12H3"/>',
  db: '<ellipse cx="12" cy="5.5" rx="8" ry="3"/><path d="M4 5.5v13c0 1.7 3.6 3 8 3s8-1.3 8-3v-13"/><path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>',
  gauge: '<path d="M4 13a8 8 0 1 1 16 0"/><path d="M12 13 15.5 8"/><circle cx="12" cy="13" r="1.6"/>',
  list: '<path d="M8 6h13M8 12h13M8 18h13"/><path d="M3 6h.01M3 12h.01M3 18h.01"/>',
};

export function Ic({ name, size, className }: { name: string; size?: number; className?: string }) {
  const d = P[name];
  if (!d) return null;
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={size ? { width: size, height: size } : undefined}
      aria-hidden
      dangerouslySetInnerHTML={{ __html: d }}
    />
  );
}
