// Follow the same visible action menus as a human; existing dialogs stay mounted.
export async function clickCanvasAction(page, label) {
  if(label==='BioArt 素材'||label==='矢量素材'){
    const source=page.getByRole('combobox',{name:'素材库来源',exact:true});
    if(!await source.isVisible())await page.locator('[data-canvas-surface]:visible').getByTitle('素材库',{exact:true}).click();
    await source.selectOption(label==='BioArt 素材'?'bioart':'bioicons');
    return;
  }
  const action=page.getByRole('button',{name:label,exact:true});
  if(!await action.isVisible()){
    const menu=label.startsWith('导出 ' )||label==='导出素材来源'?'导出':'画布工具';
    await page.getByRole('button',{name:menu,exact:true}).click();
  }
  await action.click();
}
