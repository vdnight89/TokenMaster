/**
 * 布局壳：topbar / navbar / nav-pills / content / page 结构与 round-02 原型 index.html 对齐。
 * 七页常驻挂载、按 .page/.on 切换（与原型 DOM 行为一致，含 160ms 淡入上移动效）；
 * 网关开关 / 待办完成 / 账号启停等跨页状态在此提升，模态与 toast 全局挂载。
 */
import { useEffect, useState } from "react";
import type { ReactElement } from "react";
import { PAGES, useHashPage } from "./lib/nav";
import type { PageKey } from "./lib/nav";
import { Ic } from "./lib/icons";
import { toast, ToastHost } from "./lib/toast";
import type { ModalKind } from "./lib/ui";
import { ACCOUNTS, TODOS } from "./lib/mock";
import type { Account } from "./lib/mock";
import Dash from "./pages/Dash";
import Accounts from "./pages/Accounts";
import Gateway from "./pages/Gateway";
import Usage from "./pages/Usage";
import Logs from "./pages/Logs";
import Setup from "./pages/Setup";
import Settings from "./pages/Settings";
import AddModal from "./modals/AddModal";
import ClaimModal from "./modals/ClaimModal";
import ImportModal from "./modals/ImportModal";

export default function App() {
  const [page, go] = useHashPage();
  const [gwOn, setGwOn] = useState(true);
  const [accounts, setAccounts] = useState<Account[]>(ACCOUNTS);
  const [doneTodos, setDoneTodos] = useState<ReadonlySet<string>>(() => new Set());
  const [modal, setModal] = useState<ModalKind>(null);

  /* 切页回到顶部（对应原型 nav() 里的 scrollTop = 0） */
  useEffect(() => {
    const c = document.querySelector(".content");
    if (c) c.scrollTop = 0;
  }, [page]);

  const todoCount = TODOS.length - doneTodos.size;

  const toggleGw = () => {
    const next = !gwOn;
    setGwOn(next);
    toast(
      next ? "ok" : "info",
      next ? "网关已启动" : "网关已停止",
      next ? "监听 127.0.0.1:8787 · OpenAI / Anthropic 双协议面就绪" : "进行中的流式请求已优雅收尾",
    );
  };

  const togglePower = (name: string) => {
    const a = accounts.find(x => x.name === name);
    if (!a) return;
    if (a.state === "off") {
      setAccounts(accounts.map(x => (x.name === name ? { ...x, state: "ok" as const } : x)));
      toast("ok", "账号已启用", a.name + " · 重新参与轮询");
    } else {
      setAccounts(accounts.map(x => (x.name === name ? { ...x, state: "off" as const } : x)));
      toast("info", "账号已停用", a.name + " · 已移出轮询");
    }
  };

  const markTodoDone = (id: string) => {
    setDoneTodos(prev => {
      const nx = new Set(prev);
      nx.add(id);
      return nx;
    });
  };

  const openModal = (m: Exclude<ModalKind, null>) => setModal(m);

  const pageEl: Record<PageKey, ReactElement> = {
    dash: <Dash active={page === "dash"} gwOn={gwOn} doneTodos={doneTodos} onTodoDone={markTodoDone} openModal={openModal} nav={go} />,
    accounts: <Accounts accounts={accounts} onTogglePower={togglePower} openModal={openModal} />,
    gateway: <Gateway gwOn={gwOn} onToggleGw={toggleGw} />,
    usage: <Usage />,
    logs: <Logs />,
    setup: <Setup />,
    settings: <Settings />,
  };

  return (
    <>
      {/* 顶栏拖拽区（桌面壳） */}
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

      {/* 导航 */}
      <nav className="navbar">
        <div className="brand">
          <div className="mark">TM</div>
          <div>
            <div className="name">TokenMaster</div>
            <div className="ver">15 providers · 42 accounts</div>
          </div>
        </div>
        <div className="nav-pills">
          {PAGES.map(p => (
            <button key={p.key} className={page === p.key ? "on" : ""} onClick={() => go(p.key as PageKey)}>
              {p.label}
              {p.key === "dash" && <span className="cnt">{todoCount}</span>}
            </button>
          ))}
        </div>
        <div className="nav-right">
          <div className={"gw-chip" + (gwOn ? "" : " off")}>
            <span className="dot"></span>
            <span>{gwOn ? "运行中 · :8787" : "已停止"}</span>
          </div>
          <button className="iconbtn" title="网关设置" onClick={() => go("gateway")}>
            <Ic name="gear" />
          </button>
        </div>
      </nav>

      <div className="content" style={{ flex: 1, overflowY: "auto" }}>
        {PAGES.map(p => (
          <section key={p.key} id={"page-" + p.key} className={"page" + (page === p.key ? " on" : "")}>
            {pageEl[p.key]}
          </section>
        ))}
      </div>

      {modal === "add" && <AddModal onClose={() => setModal(null)} />}
      {modal === "claim" && <ClaimModal onClose={() => setModal(null)} onSuccess={() => markTodoDone("claim")} />}
      {modal === "import" && <ImportModal onClose={() => setModal(null)} />}

      <ToastHost />
    </>
  );
}
