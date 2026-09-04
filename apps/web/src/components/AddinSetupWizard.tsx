/**
 * P6 Word 加载项 —— 首次启动向导 Modal
 *
 * 串联三步：CA 生成（自动）→ 证书信任（certutil）→ Word sideload（winreg）。
 * 完成后提示用户完全关闭 Word（含托盘）再重启以加载 Developer\Addins 注册表键。
 */

import { Modal, Steps, Button, Alert, Typography, Space, Tag } from 'antd';
import {
    CheckCircleTwoTone,
    ExclamationCircleOutlined,
    LoadingOutlined,
    WarningTwoTone,
} from '@ant-design/icons';
import { useTranslation } from 'react-i18next';
import { useAddinSetup, Step } from '../hooks/useAddinSetup';

const { Text, Paragraph } = Typography;

interface StepMeta {
    key: Step;
    titleKey: string;
    descKey: string;
}

const STEP_META: StepMeta[] = [
    {
        key: 'ca',
        titleKey: 'add Ca 生成',
        descKey: 'rcgen 生成 Autonomics Local CA 根证书 + localhost leaf 证书，落盘到 %LOCALAPPDATA%\\autonomics\\ca\\',
    },
    {
        key: 'trust',
        titleKey: 'trust 信任证书',
        descKey: 'certutil -user -addstore Root 把 CA 导入当前用户的 Trusted Root（免 UAC）。Word / Office WebView2 凭此信任 HTTPS。',
    },
    {
        key: 'sideload',
        titleKey: 'sideload 注册到 Word',
        descKey: 'winreg 写入 HKCU\\...\\Developer\\Addins\\<GUID>，指向 https://localhost:43211/manifest.xml',
    },
];

function stepIndex(step: Step): number {
    if (step === 'ca') return 0;
    if (step === 'trust') return 1;
    if (step === 'sideload') return 2;
    return 3;
}

function stepStatus(target: Step, current: Step): 'wait' | 'process' | 'finish' {
    const cur = stepIndex(current);
    const tgt = stepIndex(target);
    if (cur > tgt) return 'finish';
    if (cur === tgt) return 'process';
    return 'wait';
}

export default function AddinSetupWizard() {
    const { t } = useTranslation();
    const {
        open,
        setOpen,
        status,
        loading,
        currentStep,
        error,
        runStep,
        dismiss,
    } = useAddinSetup();

    if (!status) {
        return null;
    }

    const isDone = currentStep === 'done';
    const expiry = status.daysUntilExpiry;

    return (
        <Modal
            title={t('addins.wizard.title', 'Autonomics Word 加载项 —— 首次设置')}
            open={open}
            onCancel={dismiss}
            maskClosable={false}
            width={620}
            footer={
                isDone ? (
                    <Space>
                        <Button type="primary" onClick={() => setOpen(false)}>
                            {t('addins.wizard.close', '完成')}
                        </Button>
                    </Space>
                ) : (
                    <Space>
                        <Button onClick={dismiss}>
                            {t('addins.wizard.skip', '跳过（稍后从设置入口触发）')}
                        </Button>
                        <Button
                            type="primary"
                            loading={loading}
                            onClick={runStep}
                        >
                            {t(`addins.wizard.${currentStep}`, '执行当前步骤')}
                        </Button>
                    </Space>
                )
            }
        >
            <Space direction="vertical" size="middle" style={{ width: '100%' }}>
                <Paragraph type="secondary" style={{ marginBottom: 0 }}>
                    {t(
                        'addins.wizard.intro',
                        'Autonomics Word 加载项让你在 Word 里直接搜论文库并按 CSL 样式插引用。需要三步：生成证书、信任证书、注册到 Word。',
                    )}
                </Paragraph>

                <Steps
                    size="small"
                    current={stepIndex(currentStep)}
                    items={STEP_META.map((m) => ({
                        title: m.titleKey.split(' ').slice(1).join(' ') || m.titleKey,
                        description: m.descKey,
                        status: stepStatus(m.key, currentStep),
                    }))}
                />

                {error && (
                    <Alert
                        type="error"
                        showIcon
                        icon={<ExclamationCircleOutlined />}
                        message={t('addins.wizard.error', '步骤失败')}
                        description={error}
                    />
                )}

                {status.expiryWarning && expiry !== null && (
                    <Alert
                        type={expiry > 0 ? 'warning' : 'error'}
                        showIcon
                        icon={<WarningTwoTone twoToneColor={expiry > 0 ? '#faad14' : '#f5222d'} />}
                        message={
                            expiry > 0
                                ? t('addins.wizard.expirySoon', '证书即将过期')
                                : t('addins.wizard.expired', '证书已过期')
                        }
                        description={
                            expiry > 0
                                ? t(
                                      'addins.wizard.expirySoonDesc',
                                      'leaf 证书距过期还有 {{days}} 天。Word taskpane 将无法加载。',
                                      { days: expiry },
                                  )
                                : t(
                                      'addins.wizard.expiredDesc',
                                      'leaf 证书已过期，taskpane HTTPS 不再可信。请重新生成 CA 并重新信任（向导会自动覆盖旧证书）。',
                                  )
                        }
                    />
                )}

                {isDone ? (
                    <Alert
                        type="success"
                        showIcon
                        icon={<CheckCircleTwoTone twoToneColor="#52c41a" />}
                        message={t('addins.wizard.doneTitle', '设置完成')}
                        description={
                            <Space direction="vertical" size="small">
                                <Text>
                                    {t(
                                        'addins.wizard.doneDesc',
                                        '请完全关闭所有 Word 窗口（含托盘进程），再重启 Word 以加载 Autonomics 加载项。',
                                    )}
                                </Text>
                                <Text type="secondary" code>
                                    taskkill /F /IM WINWORD.EXE
                                </Text>
                                <Space size="small" wrap>
                                    <Tag color="green">CA 已生成</Tag>
                                    <Tag color="green">已信任</Tag>
                                    <Tag color="green">已注册到 Word</Tag>
                                </Space>
                            </Space>
                        }
                    />
                ) : (
                    loading && (
                        <Alert
                            type="info"
                            showIcon
                            icon={<LoadingOutlined />}
                            message={t('addins.wizard.running', '执行中……')}
                        />
                    )
                )}
            </Space>
        </Modal>
    );
}
