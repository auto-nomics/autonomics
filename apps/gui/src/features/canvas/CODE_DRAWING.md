# 自由编码元素

画布底部「对象编辑 → 代码绘图」打开编辑器。编写 JavaScript 函数：

```js
function draw(ctx, params, width, height) {
  const gradient = ctx.createLinearGradient(0, 0, width, height);
  gradient.addColorStop(0, params.color);
  gradient.addColorStop(1, "#ffffff");
  ctx.fillStyle = gradient;
  for (let i = 0; i < 5; i++) {
    ctx.beginPath();
    ctx.arc(40 + i * 60, height / 2, 25, 0, Math.PI * 2);
    ctx.fill();
  }
}
```

参数框填写 `{"color":"#60a5fa"}`，运行预览后插入。`ctx` 是标准 OffscreenCanvas 2D context，可使用路径、贝塞尔曲线、渐变、裁剪、变换、文字及像素操作，也可使用 JavaScript 函数、循环和数学计算。支持异步 draw，但整个运行含启动时间最多 4 秒。不提供 DOM、网络、第三方库或外部图片、字体加载。

宽高是绘图像素尺寸，背景透明。范围为 1–4096 整数，总像素最多 800 万。代码和 JSON 对象参数各最多 100000 字符。代码在一次性的 Worker 内执行，该 Worker 由 opaque-origin sandbox iframe 创建并继承禁止联网的 CSP；超时移除整个隔离环境。仅显式点击运行时执行代码，打开文档不会执行。

插入结果是一个 PNG 图片元素，可移动、缩放、分组、撤销，并使用现有保存与导出流程。代码、参数、绘图尺寸保存在 `customData.codeDrawing`（version 1），随原生源文件保存。选中单个代码元素，再打开代码绘图即可修改。更新保留元素 ID、画布位置、显示尺寸、旋转及其他元数据，替换图片并清除旧裁剪；绘图像素宽高的变化不调整画布显示尺寸。锁定或在编辑期间改变的目标会拒绝更新。未选中代码元素时创建新元素。

输出为位图；SVG 导出会嵌入图片，内部笔画不能作为原生形状单独编辑。要更改内容，请重新编辑代码。Worker 使用浏览器可用的字体，字体度量可能随环境变化。

验证：在 `apps/gui` 执行 `npm run test:canvas:code-drawing`。覆盖实际浏览器绘制、语法错误、超时、网络与 DOM 隔离、清理恢复，以及编辑器插入、参数校验、预览失效、更新、原生保存回读和 PNG 导出。
