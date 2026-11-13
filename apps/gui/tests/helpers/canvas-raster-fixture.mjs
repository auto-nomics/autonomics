import {createHash} from 'node:crypto';
export async function rasterizeFixtureImage(page,document,image){
  const raster=await page.evaluate(async url=>{const image=new Image();image.src=url;await image.decode();const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;canvas.getContext('2d').drawImage(image,0,0);return canvas.toDataURL('image/png');},document.files[image.fileId].dataURL);
  const oldFile=image.fileId;const sha=createHash('sha256').update(Buffer.from(raster.split(',')[1],'base64')).digest('hex');
  image.fileId=sha;document.files[sha]={...document.files[oldFile],id:sha,mimeType:'image/png',dataURL:raster};delete document.files[oldFile];
  image.customData.asset={id:'native-raster-fixture',author:'Native UI test',license:'fixture-only',source_url:'fixture:rasterized-svg'};image.customData.source_sha256=sha;
}
