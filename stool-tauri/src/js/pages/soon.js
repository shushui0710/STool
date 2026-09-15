/* ---------------------------------------------------------------------------
   尚未接好的页面。

   硬要求（docs/TAURI重构方案.md §3.5 第 5 条「无死胡同」）：
   空状态也必须给出**动作**，所以这里一定带「去选游戏 / 去改存档」两条出路。
   另外如实说明：功能在内核里是好的，只是界面还没搬过来；原版 stool.exe 仍可用。
--------------------------------------------------------------------------- */

const SOON_INFO = {
  extract: {
    title: "取出素材",
    sub: "把游戏里的图片、音乐、脚本拿出来，存成普通文件。",
    will: [
      "选一个输出目录，一次取出全部素材",
      "只取某一类（图片 / 音频 / 脚本）",
      "取出进度与「打开输出文件夹」按钮",
      "重新打包（把改过的文件塞回游戏）",
    ],
  },
  preview: {
    title: "看素材",
    sub: "直接浏览已经拿出来的图片、音乐和文本，不用另开看图软件。",
    will: ["左侧文件树", "右侧图片预览", "双击用系统程序打开", "音频试听"],
  },
  text: {
    title: "翻译文字",
    sub: "把游戏里的台词提成表格，翻译完再塞回游戏。",
    will: ["一句话说明两条路：脚本回填 / 运行时汉化", "提取台词到 CSV", "机翻（带重试与并发）", "把翻译写回游戏"],
  },
  runtime: {
    title: "游戏里改数值",
    sub: "游戏开着也能改，改完立刻生效，不用退出重开。",
    will: ["按名字改（推荐）", "搜数值改", "进阶：反修改保护诊断、补丁包"],
  },
  unlock: {
    title: "解锁全CG",
    sub: "一键打开所有回想与画廊，不用一格格解锁。",
    will: ["列出现在能找到的解锁途径", "写入前告知备份位置与还原方法", "写入后提供「撤销上一次」"],
  },
  mods: {
    title: "装MOD",
    sub: "安装、停用玩家做的补丁。",
    will: ["左「已安装」右「可用」两栏", "安装 MOD 文件夹", "停用 / 卸载（不改游戏自带补丁）"],
  },
  tools: {
    title: "工具箱",
    sub: "设置、诊断、帮助 —— 平时用不到，出问题才来。",
    will: ["设置（默认输出目录、并发数、机翻接口）", "体检 / 自检 / 导出诊断包", "就地帮助与常见问题"],
  },
};

function makeSoonPage(id) {
  const m = SOON_INFO[id] || { title: id, sub: "", will: [] };
  return {
    title: m.title,
    sub: m.sub,

    actions() {
      return "";
    },

    async mount(host) {
      host.innerHTML = `
        <div class="seg">
          <div class="say">
            <span class="say-mark">?</span>
            <div class="say-body">
              这一页还在搬家：Tauri 版按「选游戏 → 改存档 → …」的顺序一页一页接。<br>
              功能在内核里是好的，只是界面还没搬过来。
            </div>
          </div>
        </div>

        <div class="seg">
          <div class="card">
            <div class="card-head"><div class="card-title">这页将来做这些</div></div>
            <ul class="bullets">${m.will.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>
          </div>
        </div>

        <div class="seg">
          <div class="card">
            <div class="card-head">
              <div class="card-title">现在可以做什么</div>
              <div class="card-hint">不用卡在这一页</div>
            </div>
            <div class="row">
              <button class="btn btn-primary" id="goDetect">去「选游戏」</button>
              <button class="btn btn-ghost" id="goSave">去「改存档」</button>
            </div>
            <div class="small dim mt3">
              这页的功能现在仍然可以用原版 <span class="mono">stool.exe</span> 完成（两个版本装在一起，互不影响）。
            </div>
          </div>
        </div>
      `;
      const a = $("#goDetect", host);
      const b = $("#goSave", host);
      if (a) a.addEventListener("click", () => (location.hash = "#detect"));
      if (b) b.addEventListener("click", () => (location.hash = "#save"));
      return this;
    },
  };
}
