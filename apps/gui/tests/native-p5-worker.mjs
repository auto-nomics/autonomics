// Runs the native exploration check inside its own Xvfb/D-Bus session.
import { spawn } from 'node:child_process';
import { once } from 'node:events';
const child = spawn(process.env.BIB_P5_NATIVE_WORKER, [], { stdio: 'inherit', env: process.env });
const [code] = await once(child, 'exit');
process.exitCode = code ?? 1;
