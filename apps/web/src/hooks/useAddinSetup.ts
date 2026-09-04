/**
 * P6 Word 加载项 —— 首次启动向导状态机
 *
 * 与 Tauri 后端 commands（ca.rs / addin_sideload.rs）协作：
 * - ca_status: 查询 CA 是否已生成 + 已信任
 * - trust_ca: 生成 CA + certutil 导入 Trusted Root
 * - is_addin_sideloaded: 查询 HKCU Developer\Addins 注册表键
 * - sideload_addin: 写注册表键
 *
 * 平台门控：非 Windows 直接 needed=false（命令存在但都返回 Err/false）。
 * 用户 dismiss 后本会话不再弹（localStorage 持久化跨重启）。
 */

import { useCallback, useEffect, useState } from 'react';

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
const isWindows =
    typeof navigator !== 'undefined' && /Win/i.test(navigator.userAgent);

const DISMISS_KEY = 'autonomics.addin-setup.dismissed';

export interface AddinStatus {
    /** 4 个 CA 文件齐全 */
    generated: boolean;
    /** 已导入 Windows Trusted Root */
    trusted: boolean;
    /** HKCU 注册表键已写入 */
    sideloaded: boolean;
    /** leaf 证书距过期天数（null = 未生成 / 解析失败） */
    daysUntilExpiry: number | null;
    /** true 当距过期 ≤ 30 天 */
    expiryWarning: boolean;
}

export type Step = 'ca' | 'trust' | 'sideload' | 'done';

interface UseAddinSetupResult {
    /** Tauri + Windows + 未 sideload + 未 dismiss */
    needed: boolean;
    /** 后端查询的状态（首次加载 / 各步骤完成后刷新） */
    status: AddinStatus | null;
    /** 加载状态标志 */
    loading: boolean;
    /** 当前应执行的步骤 */
    currentStep: Step;
    /** 最近一次错误（每步独立） */
    error: string | null;
    /** 刷新后端状态 */
    refresh: () => Promise<void>;
    /** 执行当前步骤 */
    runStep: () => Promise<void>;
    /** 用户主动关闭向导（持久化 dismiss） */
    dismiss: () => void;
    /** 用户在设置页主动重新打开向导 */
    undismiss: () => void;
    /** Modal 可见性 */
    open: boolean;
    setOpen: (v: boolean) => void;
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    const { invoke: tauriInvoke } = await import('@tauri-apps/api/core');
    return tauriInvoke<T>(cmd, args);
}

function readDismissed(): boolean {
    try {
        return localStorage.getItem(DISMISS_KEY) === '1';
    } catch {
        return false;
    }
}

function writeDismissed(v: boolean) {
    try {
        if (v) localStorage.setItem(DISMISS_KEY, '1');
        else localStorage.removeItem(DISMISS_KEY);
    } catch {
        // ignore
    }
}

/**
 * 推导当前应执行的步骤（基于已知状态）。
 *
 * 优先级：CA 未生成 → ca；CA 已生成未信任 → trust；已信任未 sideload → sideload；
 * 都完成 → done。
 */
function deriveStep(s: AddinStatus): Step {
    if (!s.generated) return 'ca';
    if (!s.trusted) return 'trust';
    if (!s.sideloaded) return 'sideload';
    return 'done';
}

export function useAddinSetup(): UseAddinSetupResult {
    const [status, setStatus] = useState<AddinStatus | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [dismissed, setDismissed] = useState<boolean>(readDismissed);
    const [open, setOpen] = useState(false);

    const platformEligible = isTauri && isWindows;

    const refresh = useCallback(async () => {
        if (!platformEligible) return;
        setLoading(true);
        setError(null);
        try {
            const [ca, expiry, sideloaded] = await Promise.all([
                invoke<{ generated: boolean; trusted: boolean }>('ca_status'),
                invoke<{
                    valid: boolean;
                    daysUntilExpiry: number;
                    warning: boolean;
                }>('ca_expiry_status'),
                invoke<boolean>('is_addin_sideloaded'),
            ]);
            setStatus({
                generated: ca.generated,
                trusted: ca.trusted,
                sideloaded,
                daysUntilExpiry: ca.generated ? expiry.daysUntilExpiry : null,
                expiryWarning: ca.generated && (expiry.warning || expiry.daysUntilExpiry <= 0),
            });
        } catch (e) {
            setError(String(e));
        } finally {
            setLoading(false);
        }
    }, [platformEligible]);

    useEffect(() => {
        if (!platformEligible) return;
        refresh();
    }, [platformEligible, refresh]);

    const currentStep: Step = status ? deriveStep(status) : 'ca';

    const runStep = useCallback(async () => {
        if (!status) return;
        setError(null);
        setLoading(true);
        try {
            const step = deriveStep(status);
            if (step === 'ca' || step === 'trust') {
                // trust_ca 同时做生成 + 导入；ca 步骤也走它（即使 trusted=false 但 generated=true
                // 时调 trust_ca 会做 addstore，幂等）。
                await invoke('trust_ca');
            } else if (step === 'sideload') {
                await invoke('sideload_addin');
            }
            await refresh();
        } catch (e) {
            setError(String(e));
        } finally {
            setLoading(false);
        }
    }, [status, refresh]);

    const dismiss = useCallback(() => {
        setDismissed(true);
        writeDismissed(true);
        setOpen(false);
    }, []);

    const undismiss = useCallback(() => {
        setDismissed(false);
        writeDismissed(false);
        setOpen(true);
        refresh();
    }, [refresh]);

    const needed = platformEligible && !dismissed && status !== null && status.sideloaded === false;

    // 首次 status 加载完成且 needed 时自动打开 Modal
    useEffect(() => {
        if (needed && !open && !dismissed) {
            setOpen(true);
        }
    }, [needed, open, dismissed]);

    return {
        needed,
        status,
        loading,
        currentStep,
        error,
        refresh,
        runStep,
        dismiss,
        undismiss,
        open,
        setOpen,
    };
}
