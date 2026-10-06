/** 模态：从 zcode-pool 导入（对应原型 #modal-import / openImport） */
import { Ic } from "../lib/icons";
import { toast } from "../lib/toast";
import { ProviderBrandIcon } from "../lib/brand";
import { IMPORT_LIST } from "../lib/mock";

export default function ImportModal({ onClose }: { onClose: () => void }) {
  return (
    <div
      className="ov open"
      onClick={e => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="modal" style={{ width: "min(480px,100%)" }}>
        <div className="mh">
          <span className="chipic c-cyan">
            <Ic name="upload" />
          </span>
          从 zcode-pool 导入
          <button className="ibtn" style={{ marginLeft: "auto" }} onClick={onClose}>
            <Ic name="x" />
          </button>
        </div>
        <div className="mb">
          <div className="mnote" style={{ marginBottom: 10 }}>
            读取本机 <span className="mono">~/.zcode-pool/accounts/</span>（兼容官方 <span className="mono">enc:v1</span>{" "}
            密钥派生），转换为 TokenMaster 账号：
          </div>
          <div>
            {IMPORT_LIST.map(it => (
              <label className="kv" style={{ cursor: "pointer" }} key={it.n}>
                <input type="checkbox" className="cbx" defaultChecked style={{ borderRadius: "50%" }} />
                <ProviderBrandIcon provider="zcode" size={16} />
                <span className="grow mono" style={{ fontSize: 12 }}>
                  {it.n}
                </span>
                <span className={"bdg " + (it.warn ? "b-warn" : "b-ok")}>{it.warn ? "临期积分" : "正常"}</span>
              </label>
            ))}
          </div>
        </div>
        <div className="mf">
          <button className="btn" onClick={onClose}>
            取消
          </button>
          <button
            className="btn p"
            onClick={() => {
              onClose();
              toast("ok", "导入完成", "4 个 zcode 账号已转换入池（1 个重复已跳过）");
            }}
          >
            导入选中
          </button>
        </div>
      </div>
    </div>
  );
}
