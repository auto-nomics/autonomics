/**
 * ImagePreview.jsx
 * 图片预览组件 - 显示在聊天输入框上方
 * 用于预览用户已选择/截图的图片，在发送前提供可视化管理
 */

import React from 'react';
import { CloseOutlined } from '@ant-design/icons';
import NiceModal from '@ebay/nice-modal-react';

const ImagePreview = ({ images = [], onRemove }: { images?: any[]; onRemove?: (index: number) => void }) => {
    if (!images || images.length === 0) return null;

    const handlePreview = (image: any) => {
        void NiceModal.show('image-lightbox', {
            src: image.base64 || '',
            alt: image.name || (image.type === 'screenshot' ? '截图' : '图片'),
        });
    };

    return (
        <div
            style={{
                display: 'flex',
                flexWrap: 'wrap',
                gap: 12,
                padding: '8px 12px',
                borderBottom: '1px solid var(--border-color-light, #e0e0e0)',
            }}
        >
            {images.map((image, index) => (
                <div
                    key={index}
                    style={{ position: 'relative', padding: '6px 6px 0 0' }}
                >
                    <div
                        style={{
                            position: 'relative',
                            width: 64,
                            height: 64,
                            borderRadius: 8,
                            overflow: 'hidden',
                            border: '1px solid var(--border-color-light, #e0e0e0)',
                            backgroundColor: 'var(--bg-secondary, #f5f5f5)',
                            cursor: 'pointer',
                            transition: 'transform 0.2s, box-shadow 0.2s',
                        }}
                        onClick={() => handlePreview(image)}
                        onMouseEnter={(e) => {
                            e.currentTarget.style.transform = 'scale(1.02)';
                            e.currentTarget.style.boxShadow = '0 2px 8px rgba(0,0,0,0.15)';
                        }}
                        onMouseLeave={(e) => {
                            e.currentTarget.style.transform = 'scale(1)';
                            e.currentTarget.style.boxShadow = 'none';
                        }}
                        title="点击查看大图"
                    >
                        <img
                            src={image.base64 || ''}
                            alt={image.name || 'preview'}
                            style={{ width: '100%', height: '100%', objectFit: 'cover' }}
                        />
                        {image.type === 'screenshot' && (
                            <div style={{
                                position: 'absolute', bottom: 0, left: 0, right: 0,
                                padding: '2px 4px', backgroundColor: 'rgba(0,0,0,0.5)',
                                color: 'var(--text-inverted)', fontSize: 10, textAlign: 'center',
                            }}>
                                截图
                            </div>
                        )}
                    </div>

                    <button
                        onClick={(e) => {
                            e.stopPropagation();
                            onRemove?.(index);
                        }}
                        style={{
                            position: 'absolute', top: 0, right: 0, width: 18, height: 18,
                            padding: 0, border: 'none', borderRadius: '50%',
                            backgroundColor: 'rgba(0, 0, 0, 0.45)', cursor: 'pointer',
                            display: 'flex', alignItems: 'center', justifyContent: 'center',
                            transition: 'background-color 0.2s',
                        }}
                        onMouseEnter={(e) => { e.currentTarget.style.backgroundColor = 'rgba(0, 0, 0, 0.65)'; }}
                        onMouseLeave={(e) => { e.currentTarget.style.backgroundColor = 'rgba(0, 0, 0, 0.45)'; }}
                        title="移除图片"
                    >
                        <CloseOutlined style={{ fontSize: 10, color: 'var(--text-inverted)' }} />
                    </button>
                </div>
            ))}
        </div>
    );
};

export default ImagePreview;
