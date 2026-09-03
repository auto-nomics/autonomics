/**
 * 后端进程死亡全屏遮罩（Windows 分发计划 M4）
 *
 * desktop 端 spawn_death_monitor（lib.rs）检测到 jayread-server 退出时
 * emit `backend-exited`（payload: { message, logTail }），同时弹原生对话框。
 * 原生框说明「发生了什么」；此遮罩负责盖住 webview 内的僵死 UI——后端死后
 * 所有 API 请求只会 30s 超时静默失败，不遮就是白屏假活。
 *
 * 仅 Tauri 模式生效；文案与原生对话框保持中文一致（Rust 侧同款，不走 i18n）。
 */
import { useEffect, useState } from 'react';
import { Button, Result } from 'antd';

interface BackendExitPayload {
  message: string;
  logTail: string;
}

const LOG_PRE_STYLE: React.CSSProperties = {
  textAlign: 'left',
  maxWidth: 640,
  maxHeight: 200,
  overflow: 'auto',
  margin: '0 auto',
  padding: '8px 12px',
  background: 'rgba(0, 0, 0, 0.04)',
  borderRadius: 6,
  fontSize: 12,
  lineHeight: 1.6,
  whiteSpace: 'pre-wrap',
  wordBreak: 'break-all',
};

async function closeMainWindow(): Promise<void> {
  // capabilities 已含 core:window:allow-close；主窗口关闭 = 应用退出
  // （RunEvent::Exit → runtime 清理 server/PG）
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().close();
}

export default function BackendExitOverlay() {
  const [exit, setExit] = useState<BackendExitPayload | null>(null);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;

    let unlisten: (() => void) | undefined;
    let disposed = false;
    import('@tauri-apps/api/event')
      .then(({ listen }) =>
        listen<BackendExitPayload>('backend-exited', (e) => setExit(e.payload)),
      )
      .then((fn) => {
        // StrictMode 双挂载：卸载后 promise 才 resolve 时立即释放
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((err) => console.warn('[BackendExitOverlay] 监听失败:', err));

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  if (!exit) return null;

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 10000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'rgba(0, 0, 0, 0.45)',
      }}
    >
      <Result
        status="error"
        title="后端服务已退出"
        subTitle={exit.message}
        extra={[
          <pre key="log" style={LOG_PRE_STYLE}>
            {exit.logTail || '（无日志输出）'}
          </pre>,
          <Button key="exit" type="primary" onClick={() => void closeMainWindow()}>
            退出应用
          </Button>,
        ]}
      />
    </div>
  );
}
