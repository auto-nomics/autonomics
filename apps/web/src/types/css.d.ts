/**
 * CSS 模块类型声明
 *
 * 为 CSS Modules 提供类型支持，允许导入 .module.css 文件并获得类型提示。
 */

declare module '*.module.css' {
  /** CSS 模块的类名映射，键为类名，值为经过 CSS Modules 处理后的唯一类名字符串 */
  const classes: { readonly [key: string]: string };
  export default classes;
}
