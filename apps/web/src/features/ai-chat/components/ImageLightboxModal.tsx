/**
 * ImageLightboxModal.jsx
 * 图片大图预览弹窗组件
 * 基于 Ant Design Modal 实现，用于全屏查看图片
 * 无标题栏，仅保留关闭按钮，图片居中显示
 * 供聊天记录（ChatImageLightbox）和输入框预览（ImagePreview）复用
 */

import React from 'react';
import { Modal } from 'antd';
import NiceModal, { useModal } from '@ebay/nice-modal-react';

/**
 * 图片灯箱弹窗组件
 * 全屏居中显示大图，支持点击遮罩层或关闭按钮关闭
 */
const ImageLightboxModal = NiceModal.create(({ src, alt = '' }: { src: string; alt?: string }) => {
    const modal = useModal();

    return (
        <Modal
            open={modal.visible}
            title={null}
            closable
            footer={null}
            onCancel={() => modal.hide()}
            centered
            width="auto"
            styles={{
                body: {
                    padding: 0,
                    display: 'flex',
                    justifyContent: 'center',
                    alignItems: 'center',
                    maxHeight: '80vh',
                    overflow: 'auto',
                },
            }}
        >
            <img
                alt={alt}
                src={src}
                style={{
                    maxWidth: '100%',
                    maxHeight: '75vh',
                    objectFit: 'contain',
                }}
            />
        </Modal>
    );
});

export default ImageLightboxModal;
