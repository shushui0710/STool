/* ---------------------------------------------------------------------------
   与 Rust 侧的唯一通道。

   刻意做得很薄：一个调用函数 + 一处错误规整。这样页面代码里不会到处 try/catch，
   也不会各页自己解释「失败长什么样」——失败恒定返回一句「原因 + 修法」的文本。
--------------------------------------------------------------------------- */

const Tauri = (() => {
  const api = () => window.__TAURI__;

  /** 调用 Rust 命令。永远返回 { ok:true, data } 或 { ok:false, err }。 */
  async function call(cmd, args = {}) {
    const t = api();
    if (!t) {
      return {
        ok: false,
        err: "没连上 Rust 后端（window.__TAURI__ 不存在）。\n改法：用 STool 窗口打开，而不是直接用浏览器打开这个页面。",
      };
    }
    try {
      return { ok: true, data: await t.core.invoke(cmd, args) };
    } catch (e) {
      return { ok: false, err: typeof e === "string" ? e : (e && e.message) || String(e) };
    }
  }

  /** 当前窗口句柄（用于系统级拖动事件）。拿不到就返回 null。 */
  function currentWin() {
    try {
      return api().webviewWindow.getCurrentWebviewWindow();
    } catch (_) {
      return null;
    }
  }

  /**
   * 订阅 Rust 侧推来的事件（长任务进度走这里）。
   *
   * 返回取消订阅的函数；拿不到事件通道时返回一个空函数 —— 这样调用方
   * 不必到处判空（进度条画不出来，但主流程照常）。
   */
  async function on(event, cb) {
    const t = api();
    if (!t || !t.event) return () => {};
    try {
      return await t.event.listen(event, (e) => cb(e.payload));
    } catch (_) {
      return () => {};
    }
  }

  return { call, currentWin, on };
})();

/** 跨页共享的少量状态。刻意不做全局 store —— 只有这几件事需要跨页记住。 */
const Store = {
  /** 当前选中的游戏根目录 */
  gameRoot: null,
  /** 最近一次检测结果（DetectOut） */
  detect: null,
};
