/* —— ⚠️ 本文件是《从真实游戏里抠出来的原始引擎代码》，不是手写的。
 * 来源：NTRdemic（Electron + TyranoScript v6）/resources/app.asar
 *   → 条目 /tyrano/plugins/kag/kag.tag.js
 *   → 函数 buildMessageHTML 的逐字节 slice
 * 片段：1389 字节，sha256 673ae16956b4fa0609cc670f6d4e63de199a9baef13091a5b2a1e7082c5b84f1
 * 属性：这就是产出「逐字 <span class="char" style="opacity:0">」的那段真代码——它让 DOM 节点级替换永远命中不了正文，
 *   是「角色名翻成中文、正文整片日文」这类 bug 的真因。
 * 用途：scripts/verify_hook_tyrano.cjs 把它当「真渲染器」，配合真 hook 断言正文真的变中文。
 * 维护：别手改（改了就不再是「真机证据」）；游戏更新后重抠（重跑抽取脚本并更新 sha256）。
 * 包装：只加了 module.exports = {  ...  } 这一层容器，函数体逐字节未动。
 */
module.exports = {buildMessageHTML:function(message_str,should_use_inline_block=!0){let message_html="";const word_nobreak_list=this.kag.stat.word_nobreak_list||[],should_check_word_break=word_nobreak_list.length>0;let is_escaping=!1;if(should_check_word_break){const escape_char=this.getEscapeChar(message_str);word_nobreak_list.forEach((word=>{const reg=new RegExp(word,"g");message_str=message_str.replace(reg,escape_char+word+escape_char)}))}for(let i=0;i<message_str.length;i++){let c=message_str.charAt(i);if(should_check_word_break&&undefined===c)if(is_escaping){is_escaping=!1;message_html+="</span>"}else{is_escaping=!0;message_html+='<span style="display: inline-block;">'}else{if(""!=this.kag.stat.ruby_str){c=`<ruby><rb>${c}</rb><rt>${this.kag.stat.ruby_str}</rt></ruby>`;this.kag.stat.ruby_str=""}if(" "==c)message_html+=`<span class="char" style="opacity:0">${c}</span>`;else{if(1==this.kag.stat.mark){c=`<mark style="${this.kag.stat.style_mark}">${c}</mark>`}else 2==this.kag.stat.mark&&(this.kag.stat.mark=0);this.kag.tmp.is_individual_decoration?this.kag.tmp.is_text_stroke?message_html+=this.buildTextStrokeChar(c,this.kag.stat.font.edge):message_html+=this.buildTextShadowChar(c,this.kag.stat.font.edge):message_html+=should_use_inline_block?`<span class="char" style="opacity:0;display:inline-block;">${c}</span>`:`<span class="char" style="opacity:0">${c}</span>`}}}return message_html}};
