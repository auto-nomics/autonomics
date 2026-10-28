// Runs inside its own Xvfb/D-Bus session; drives the real GTK save dialog.
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { join } from 'node:path';
const child=spawn(process.env.BIB_P5_NATIVE_WORKER,[],{stdio:'inherit',env:process.env});
let dialogHandled=false;
const timer=setInterval(()=>{
  if(dialogHandled)return;
  try {
    const id=execFileSync('xdotool',['search','--onlyvisible','--name','^Save export$'],{encoding:'utf8',stdio:['ignore','pipe','ignore']}).trim().split('\n')[0];
    if(!id)return;
    execFileSync('xdotool',['windowfocus','--sync',id]);
    execFileSync('xdotool',['key','--clearmodifiers','ctrl+l']);
    execFileSync('xdotool',['key','--clearmodifiers','ctrl+a']);
    execFileSync('xdotool',['type','--clearmodifiers','--delay','1',join(process.env.BIB_P5_NATIVE_ROOT,'native-snapshot.json')]);
    execFileSync('xdotool',['key','--clearmodifiers','Return']);
    dialogHandled=true;
  } catch { /* No dialog yet. */ }
},250);
try {const [code]=await once(child,'exit');if(!dialogHandled)throw new Error('Actual GTK save dialog was never driven');process.exitCode=code??1;}
finally {clearInterval(timer);}
