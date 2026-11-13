// Follow the same visible action menus as a human; existing dialogs stay mounted.
export async function clickCanvasAction(page, label) {
  if(label==='BioArt 素材'||label==='矢量素材'){
    const source=page.getByRole('combobox',{name:'素材库来源',exact:true});
    if(!await source.isVisible())await page.locator('[data-canvas-surface]:visible').getByTitle('素材库',{exact:true}).click();
    await source.selectOption(label==='BioArt 素材'?'bioart':'bioicons');
    return;
  }
  const action=page.locator('[data-canvas-surface]:visible [data-canvas-action-dock]').getByRole('button',{name:label,exact:true,includeHidden:true});
  if(!await action.isVisible()){
    const group=action.locator('xpath=ancestor::*[@data-canvas-action-menu][1]');
    const menu=await group.getAttribute('data-canvas-action-menu');
    if(!menu)throw new Error(`No canvas action group for ${label}`);
    await group.getByRole('button',{name:menu,exact:true}).click();
  }
  await action.click();
}
