/** 模态：领取套餐 · 验证码载体（对应原型 #modal-claim / openClaim） */
import { Ic } from "../lib/icons";
import { toast } from "../lib/toast";

export default function ClaimModal({ onClose, onSuccess }: { onClose: () => void; onSuccess: () => void }) {
  return (
    <div
      className="ov open"
      onClick={e => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="modal" style={{ width: "min(430px,100%)" }}>
        <div className="mh">
          <span className="chipic c-violet">
            <Ic name="gift" />
          </span>
          领取套餐
          <button className="ibtn" style={{ marginLeft: "auto" }} onClick={onClose}>
            <Ic name="x" />
          </button>
        </div>
        <div className="mb">
          <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 12, padding: "18px 0 10px" }}>
            <span className="chipic c-blue" style={{ width: 44, height: 44, borderRadius: 12 }}>
              <Ic name="shield" size={22} />
            </span>
            <div style={{ fontWeight: 700, fontSize: 14 }}>验证码载体运行中</div>
            <div className="mnote" style={{ textAlign: "center", maxWidth: 330 }}>
              已按需拉起 WebView 过码环境（无感验证），成功后自动提交 <span className="mono">billing/claim</span>
              ；8 秒无感失败将亮出窗口转人工。
            </div>
            <div className="mono" style={{ fontSize: 11, color: "var(--t4)" }}>
              account: zc-moonton@z.ai · plan: GLM Coding Pro 14d
            </div>
            <button
              className="btn p"
              style={{ marginTop: 4 }}
              onClick={() => {
                onClose();
                onSuccess();
                toast("ok", "套餐领取成功", "GLM Coding Pro · 14 天已入账 zc-moonton@z.ai");
              }}
            >
              <Ic name="check" />
              模拟过码成功
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
