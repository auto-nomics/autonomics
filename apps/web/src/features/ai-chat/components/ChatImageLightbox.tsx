/**
 * ChatImageLightbox.jsx
 * 聊天消息中的图片灯箱组件
 * 单击图片弹出全屏大图预览弹窗（通过 NiceModal 命令式调用）
 */

import React, { useCallback } from 'react';
import NiceModal from '@ebay/nice-modal-react';

export default function ChatImageLightbox({ src, alt = '', thumbnail = false, imgProps = {} as Record<string, any> }: { src?: string; alt?: string; thumbnail?: boolean; imgProps?: Record<string, any> }) {
    const show = useCallback(() => {
        if (src) {
            void NiceModal.show('image-lightbox', { src, alt: alt || 'image' });
        }
    }, [src, alt]);

    if (!src) return null;

    return (
        <span
            role="button"
            tabIndex={0}
            onClick={(e) => {
                e.stopPropagation();
                show();
            }}
            onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    show();
                }
            }}
            style={{
                cursor: 'pointer',
                display: 'inline-block',
                maxWidth: thumbnail ? undefined : '100%',
                lineHeight: 0,
            }}
            title="点击查看大图"
        >
            {thumbnail ? (
                <img
                    src={src}
                    alt={alt}
                    style={{
                        maxWidth: 150,
                        maxHeight: 150,
                        borderRadius: 6,
                        objectFit: 'cover',
                        display: 'block',
                    }}
                />
            ) : (
                <img {...imgProps} src={src} alt={alt} style={{ maxWidth: '100%', height: 'auto', ...imgProps.style }} />
            )}
        </span>
    );
}
