/**
 * T5.10 i18n 双语：zh/en 词条表 + t()。
 *
 * 用法：
 *   import { t, useLang } from "./lib/i18n";
 *   t("nav.dash")                          // 静态调用（默认 zh）
 *   const [lang, setLang] = useLang();     // React hook（触发重渲染）
 *   t("nav.dash", lang)                    // 指定语言
 *
 * 词条按命名空间组织：nav.* / dash.* / accounts.* / gateway.* / usage.* /
 * logs.* / setup.* / settings.* / common.* / toast.* / modal.*
 */
import { useEffect, useState } from "react";

export type Lang = "zh" | "en";

// ── 中文（默认） ──
const zh: Record<string, string> = {
  // nav
  "nav.dash": "仪表盘",
  "nav.accounts": "账号",
  "nav.gateway": "网关",
  "nav.usage": "用量",
  "nav.logs": "日志",
  "nav.setup": "接入",
  "nav.settings": "设置",

  // common
  "common.on": "已启用",
  "common.off": "已停用",
  "common.start": "启动",
  "common.stop": "停止",
  "common.refresh": "刷新",
  "common.save": "保存",
  "common.cancel": "取消",
  "common.confirm": "确认",
  "common.delete": "删除",
  "common.edit": "编辑",
  "common.add": "添加",
  "common.search": "搜索",
  "common.all": "全部",
  "common.none": "无",
  "common.loading": "加载中…",
  "common.empty": "暂无数据",
  "common.copy": "复制",
  "common.copied": "已复制",
  "common.close": "关闭",
  "common.retry": "重试",
  "common.yes": "是",
  "common.no": "否",

  // dash
  "dash.title": "仪表盘",
  "dash.todayRequests": "今日请求",
  "dash.todayTokens": "今日 Token",
  "dash.activeAccounts": "活跃账号",
  "dash.gatewayStatus": "网关状态",
  "dash.todos": "待办事项",
  "dash.throughput": "吞吐",
  "dash.poolGauges": "账号池状态",
  "dash.errorDist": "错误分布",
  "dash.recentRequests": "最近请求",
  "dash.noTodos": "所有任务已完成 🎉",
  "dash.markDone": "标记完成",

  // accounts
  "accounts.title": "账号管理",
  "accounts.addAccount": "添加账号",
  "accounts.importPool": "导入 zcode-pool",
  "accounts.claim": "领取",
  "accounts.enabled": "已启用",
  "accounts.disabled": "已停用",
  "accounts.balance": "余额",
  "accounts.expires": "到期",
  "accounts.models": "可用模型",
  "accounts.statusCards": "状态卡片",
  "accounts.filterAll": "全部",
  "accounts.filterActive": "正常",
  "accounts.filterLimited": "限流中",
  "accounts.filterExpired": "已过期",
  "accounts.deleteConfirm": "确定要删除此账号？",

  // gateway
  "gateway.title": "网关设置",
  "gateway.port": "监听端口",
  "gateway.baseUrl": "Base URL",
  "gateway.apiKey": "网关密钥",
  "gateway.mapping": "模型映射",
  "gateway.strategy": "选号策略",
  "gateway.strategy.expireFirst": "先到期优先",
  "gateway.strategy.mostRemaining": "剩余最多",
  "gateway.topology": "拓扑图",
  "gateway.running": "运行中",
  "gateway.stopped": "已停止",
  "gateway.portHint": "重启后生效",

  // usage
  "usage.title": "用量统计",
  "usage.today": "今日",
  "usage.week": "本周",
  "usage.month": "本月",
  "usage.totalTokens": "总 Token",
  "usage.inputTokens": "输入",
  "usage.outputTokens": "输出",
  "usage.byProvider": "按 Provider",
  "usage.byModel": "按模型",
  "usage.byAccount": "按账号",
  "usage.details": "明细",

  // logs
  "logs.title": "请求日志",
  "logs.filter": "过滤",
  "logs.monitor": "实时监控",
  "logs.method": "方法",
  "logs.path": "路径",
  "logs.status": "状态",
  "logs.duration": "耗时",
  "logs.model": "模型",
  "logs.account": "账号",
  "logs.tokens": "Token",
  "logs.switchTrail": "换号轨迹",

  // setup
  "setup.title": "接入指南",
  "setup.openaiCompat": "OpenAI 兼容客户端",
  "setup.claudeCode": "Claude Code",
  "setup.cline": "Cline",
  "setup.custom": "自定义",
  "setup.modelTable": "可用模型表",

  // settings
  "settings.title": "设置",
  "settings.lang": "界面语言",
  "settings.lang.zh": "中文",
  "settings.lang.en": "English",
  "settings.proxy": "出站代理",
  "settings.proxy.global": "全局代理",
  "settings.proxy.provider": "Provider 级",
  "settings.proxy.account": "账号级",
  "settings.proxy.none": "不使用代理",
  "settings.about": "关于",
  "settings.version": "版本",
  "settings.acks": "致谢",

  // toast
  "toast.saved": "已保存",
  "toast.copied": "已复制到剪贴板",
  "toast.deleted": "已删除",
  "toast.gatewayStarted": "网关已启动",
  "toast.gatewayStopped": "网关已停止",
  "toast.accountAdded": "账号已添加",
  "toast.claimSuccess": "领取成功",
  "toast.claimAlready": "今日已领取",
};

// ── English ──
const en: Record<string, string> = {
  "nav.dash": "Dashboard",
  "nav.accounts": "Accounts",
  "nav.gateway": "Gateway",
  "nav.usage": "Usage",
  "nav.logs": "Logs",
  "nav.setup": "Setup",
  "nav.settings": "Settings",

  "common.on": "Enabled",
  "common.off": "Disabled",
  "common.start": "Start",
  "common.stop": "Stop",
  "common.refresh": "Refresh",
  "common.save": "Save",
  "common.cancel": "Cancel",
  "common.confirm": "Confirm",
  "common.delete": "Delete",
  "common.edit": "Edit",
  "common.add": "Add",
  "common.search": "Search",
  "common.all": "All",
  "common.none": "None",
  "common.loading": "Loading…",
  "common.empty": "No data",
  "common.copy": "Copy",
  "common.copied": "Copied",
  "common.close": "Close",
  "common.retry": "Retry",
  "common.yes": "Yes",
  "common.no": "No",

  "dash.title": "Dashboard",
  "dash.todayRequests": "Requests Today",
  "dash.todayTokens": "Tokens Today",
  "dash.activeAccounts": "Active Accounts",
  "dash.gatewayStatus": "Gateway Status",
  "dash.todos": "To-Do",
  "dash.throughput": "Throughput",
  "dash.poolGauges": "Pool Status",
  "dash.errorDist": "Error Distribution",
  "dash.recentRequests": "Recent Requests",
  "dash.noTodos": "All done 🎉",
  "dash.markDone": "Mark Done",

  "accounts.title": "Account Management",
  "accounts.addAccount": "Add Account",
  "accounts.importPool": "Import zcode-pool",
  "accounts.claim": "Claim",
  "accounts.enabled": "Enabled",
  "accounts.disabled": "Disabled",
  "accounts.balance": "Balance",
  "accounts.expires": "Expires",
  "accounts.models": "Models",
  "accounts.statusCards": "Status Cards",
  "accounts.filterAll": "All",
  "accounts.filterActive": "Active",
  "accounts.filterLimited": "Rate Limited",
  "accounts.filterExpired": "Expired",
  "accounts.deleteConfirm": "Delete this account?",

  "gateway.title": "Gateway Settings",
  "gateway.port": "Port",
  "gateway.baseUrl": "Base URL",
  "gateway.apiKey": "API Key",
  "gateway.mapping": "Model Mapping",
  "gateway.strategy": "Selection Strategy",
  "gateway.strategy.expireFirst": "Expire First",
  "gateway.strategy.mostRemaining": "Most Remaining",
  "gateway.topology": "Topology",
  "gateway.running": "Running",
  "gateway.stopped": "Stopped",
  "gateway.portHint": "Takes effect after restart",

  "usage.title": "Usage Statistics",
  "usage.today": "Today",
  "usage.week": "This Week",
  "usage.month": "This Month",
  "usage.totalTokens": "Total Tokens",
  "usage.inputTokens": "Input",
  "usage.outputTokens": "Output",
  "usage.byProvider": "By Provider",
  "usage.byModel": "By Model",
  "usage.byAccount": "By Account",
  "usage.details": "Details",

  "logs.title": "Request Logs",
  "logs.filter": "Filter",
  "logs.monitor": "Live Monitor",
  "logs.method": "Method",
  "logs.path": "Path",
  "logs.status": "Status",
  "logs.duration": "Duration",
  "logs.model": "Model",
  "logs.account": "Account",
  "logs.tokens": "Tokens",
  "logs.switchTrail": "Switch Trail",

  "setup.title": "Setup Guide",
  "setup.openaiCompat": "OpenAI Compatible",
  "setup.claudeCode": "Claude Code",
  "setup.cline": "Cline",
  "setup.custom": "Custom",
  "setup.modelTable": "Available Models",

  "settings.title": "Settings",
  "settings.lang": "Language",
  "settings.lang.zh": "中文",
  "settings.lang.en": "English",
  "settings.proxy": "Outbound Proxy",
  "settings.proxy.global": "Global",
  "settings.proxy.provider": "Per Provider",
  "settings.proxy.account": "Per Account",
  "settings.proxy.none": "No Proxy",
  "settings.about": "About",
  "settings.version": "Version",
  "settings.acks": "Acknowledgments",

  "toast.saved": "Saved",
  "toast.copied": "Copied to clipboard",
  "toast.deleted": "Deleted",
  "toast.gatewayStarted": "Gateway started",
  "toast.gatewayStopped": "Gateway stopped",
  "toast.accountAdded": "Account added",
  "toast.claimSuccess": "Claim successful",
  "toast.claimAlready": "Already claimed today",
};

const DICTS: Record<Lang, Record<string, string>> = { zh, en };

/** 当前语言（模块级，被 React hook 同步） */
let currentLang: Lang = "zh";

/** 取词条；缺失时回退中文再回退 key 本身。 */
export function t(key: string, lang?: Lang): string {
  const l = lang ?? currentLang;
  return DICTS[l][key] ?? DICTS.zh[key] ?? key;
}

/** React hook：读写当前语言（同步模块级 + localStorage + <html lang>）。 */
export function useLang(): [Lang, (l: Lang) => void] {
  const [lang, setLangState] = useState<Lang>(() => {
    const saved = localStorage.getItem("tm-lang");
    return saved === "en" || saved === "zh" ? saved : "zh";
  });
  useEffect(() => {
    currentLang = lang;
    localStorage.setItem("tm-lang", lang);
    document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  }, [lang]);
  const setLang = (l: Lang) => setLangState(l);
  return [lang, setLang];
}

/** 批量取词条（简写）。 */
export function tf(keys: string[], lang?: Lang): string[] {
  return keys.map((k) => t(k, lang));
}
