/**
 * ImageUploader.jsx
 * 图片上传组件
 * 提供上传本地图片按钮
 *
 * 上传功能说明：
 * - 使用隐藏的 <input type="file"> 实现文件选择
 * - 支持 image/* 类型的文件，限制大小 10MB
 * - 读取为 base64 Data URL 后通过回调传出
 */

import React, { useRef, useCallback } from 'react';
import { PictureOutlined } from '@ant-design/icons';
import { Button, App } from 'antd';

/**
 * 图片上传组件
 * 提供上传图片按钮，通过回调将图片数据传给父组件
 *
 * @param {Object} props
 * @param {Function} props.onImageSelect - 图片选择完成后的回调函数
 *   参数: { base64: string, file?: File, type: 'upload', name: string }
 * @param {Object} [props.iconStyle={}] - 按钮图标的自定义样式
 * @param {boolean} [props.disabled=false] - 是否禁用按钮（如当前模型不支持图片）
 * @param {string} [props.disabledTitle=''] - 禁用时的提示信息
 */
const ImageUploader = ({ onImageSelect, iconStyle = {}, disabled = false, disabledTitle = '' }: { onImageSelect?: (data: any) => void; iconStyle?: Record<string, any>; disabled?: boolean; disabledTitle?: string }) => {
    const { message } = App.useApp();
    const fileInputRef = useRef<HTMLInputElement>(null);

    /**
     * 处理文件选择事件
     * 验证文件类型和大小后，读取为 base64 并通过回调传出
     */
    const handleFileChange = useCallback((event: React.ChangeEvent<HTMLInputElement>) => {
        if (disabled) return;
        const file = event.target.files?.[0];
        if (!file) return;

        if (!file.type.startsWith('image/')) {
            message.error('请选择图片文件');
            return;
        }

        if (file.size > 10 * 1024 * 1024) {
            message.error('图片大小不能超过 10MB');
            return;
        }

        const reader = new FileReader();
        reader.onload = (e) => {
            const base64 = e.target?.result;
            if (base64 && onImageSelect) {
                onImageSelect({
                    base64,
                    file,
                    type: 'upload',
                    name: file.name
                });
            }
        };
        reader.onerror = () => {
            message.error('读取图片失败');
        };
        reader.readAsDataURL(file);

        event.target.value = '';
    }, [onImageSelect, disabled]);

    /**
     * 触发文件选择对话框
     */
    const handleUploadClick = useCallback(() => {
        if (disabled) {
            if (disabledTitle) message.warning(disabledTitle);
            return;
        }
        fileInputRef.current?.click();
    }, [disabled, disabledTitle]);

    return (
        <>
            <input
                ref={fileInputRef}
                type="file"
                accept="image/*"
                onChange={handleFileChange}
                style={{ display: 'none' }}
            />

            <Button
                type="text"
                icon={<PictureOutlined />}
                onClick={handleUploadClick}
                title={disabled ? (disabledTitle || '当前模型不支持带图') : '上传图片'}
                disabled={disabled}
                style={{ ...iconStyle, opacity: disabled ? 0.45 : 1 }}
            />
        </>
    );
};

export default React.memo(ImageUploader);
