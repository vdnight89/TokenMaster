import { PAGES, useHashPage, type PageKey } from "./lib/nav";

/**
 * 布局壳：结构/类名与 round-02 原型 index.html 对齐
 * （topbar / navbar / nav-pills / content / page）。
 * 各页面内容随后续任务从原型移植。
 */
export default function App() {
  const [page, go] = useHashPage();

  return (
    <>
      <div className="topbar">
        <div className="traffic">
          <i></i>
          <i></i>
          <i></i>
        </div>
        <span>TokenMaster</span>
        <span className="grow"></span>
        <span className="mono">v0.1.0-dev</span>
      </div>

      <nav className="navbar">
        <div className="brand">
          <div className="mark">TM</div>
          <div>
            <div className="name">TokenMaster</div>
            <div className="ver">15 providers</div>
          </div>
        </div>
        <div className="nav-pills">
          {PAGES.map((p) => (
            <button key={p.key} className={page === p.key ? "on" : ""} onClick={() => go(p.key as PageKey)}>
              {p.label}
            </button>
          ))}
        </div>
        <div className="nav-right">
          <div className="gw-chip">
            <span className="dot"></span>
            <span>运行中 · :8787</span>
          </div>
        </div>
      </nav>

      <div className="content">
        {PAGES.map((p) => (
          <div key={p.key} id={`page-${p.key}`} className={`page${page === p.key ? " on" : ""}`}>
            <div className="page-wrap">
              <div className="page-h">
                <h1>{p.label}</h1>
              </div>
            </div>
          </div>
        ))}
      </div>
    </>
  );
}
