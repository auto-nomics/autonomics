/**
 * Autonomics 附件列表组件
 *
 * 在论文列表页展开论文行时，显示该论文的所有附件（包括主 PDF）。
 * 所有文件统一作为附件展示，不再区分"主PDF"和"附件"。
 * 主 PDF 通过 is_primary 标识，显示解析状态标签。
 *
 * 布局结构：
 * ┌────────────────────────────────────────────────────────────┐
 * │  [缩进][PDF图标] main-paper.pdf   ✅MinerU  [打开阅读][删除] │
 * │  [缩进][PDF图标] supplementary.pdf   245 KB  [打开阅读][删除] │
 * │  [缩进][DOCX图标] appendix.docx      180 KB  [下载][删除]     │
 * │  [缩进][PNG图标]  figure-1.png       340 KB  [预览][删除]     │
 * └────────────────────────────────────────────────────────────┘
 *
 * 附件上传方式：将文件从桌面拖拽到论文行即可上传为附件。
 *
 * Props:
 * - paper: 论文对象（PaperResponse），用于读取 parse_status 等解析状态
 * - attachments: 附件数组（AttachmentResponse[]），每个附件包含 is_primary 字段
 * - loading: 是否正在加载附件列表
 * - onDelete: 删除回调函数，接收附件 ID
 * - onOpenPdf: 打开 PDF 回调函数，接收附件 ID
 * - onSetPrimary: 设为主 PDF 回调函数，接收附件 ID（用于切换论文的主 PDF）
 */

import React from 'react'; // 导入 React
import { useTranslation } from 'react-i18next'; // 导入 i18n 翻译钩子
import i18next from 'i18next'; // 导入 i18next 用于模块级常量
// 导入 Ant Design 组件
import { Button, Tag, Tooltip, Spin, Dropdown } from 'antd';
// 导入 Ant Design 图标组件
import {
  FilePdfOutlined,    // PDF 文件图标（红色调）
  FileWordOutlined,   // Word 文档图标（蓝色调）
  FileExcelOutlined,  // Excel 表格图标（绿色调）
  FilePptOutlined,    // PowerPoint 图标（橙色调）
  FileImageOutlined,  // 图片文件图标（紫色调）
  SoundOutlined,      // 音频文件图标
  PlayCircleOutlined, // 视频文件图标
  FileZipOutlined,    // 压缩包图标
  CodeOutlined,       // 代码文件图标
  BookOutlined,       // 电子书图标
  DatabaseOutlined,   // 数据文件图标
  FileTextOutlined,   // 纯文本图标
  GlobalOutlined,     // 网页图标
  FileOutlined,       // 默认文件图标
  DeleteOutlined,     // 删除图标
  DownloadOutlined,   // 下载图标
  EyeOutlined,        // 预览（眼睛）图标
  CheckCircleOutlined, // 成功对勾图标
  LoadingOutlined,    // 加载中图标
  StarOutlined,       // 星标图标（用于"设为主 PDF"菜单项）
} from '@ant-design/icons';

// 导入附件工具函数
import {
  getFileCategory,         // 根据扩展名获取文件分类
  getFileColor,            // 根据分类获取主题颜色
  formatFileSize,          // 格式化文件大小
} from '../../../../utils/attachmentUtils';
// 导入附件 API：获取附件下载 URL
import { getAttachmentUrl } from '../../../../services/attachmentsApi';

// ========== 图标名称到组件的映射表 ==========
// 将字符串图标名称映射为实际的 React 组件实例
// 这样可以根据文件分类动态选择图标组件
const ICON_COMPONENT_MAP: Record<string, (color: any) => JSX.Element> = {
  FilePdfOutlined: (color: any) => <FilePdfOutlined style={{ color, fontSize: 16 }} />,
  FileWordOutlined: (color: any) => <FileWordOutlined style={{ color, fontSize: 16 }} />,
  FileExcelOutlined: (color: any) => <FileExcelOutlined style={{ color, fontSize: 16 }} />,
  FilePptOutlined: (color: any) => <FilePptOutlined style={{ color, fontSize: 16 }} />,
  FileImageOutlined: (color: any) => <FileImageOutlined style={{ color, fontSize: 16 }} />,
  SoundOutlined: (color: any) => <SoundOutlined style={{ color, fontSize: 16 }} />,
  PlayCircleOutlined: (color: any) => <PlayCircleOutlined style={{ color, fontSize: 16 }} />,
  FileZipOutlined: (color: any) => <FileZipOutlined style={{ color, fontSize: 16 }} />,
  CodeOutlined: (color: any) => <CodeOutlined style={{ color, fontSize: 16 }} />,
  BookOutlined: (color: any) => <BookOutlined style={{ color, fontSize: 16 }} />,
  DatabaseOutlined: (color: any) => <DatabaseOutlined style={{ color, fontSize: 16 }} />,
  FileTextOutlined: (color: any) => <FileTextOutlined style={{ color, fontSize: 16 }} />,
  GlobalOutlined: (color: any) => <GlobalOutlined style={{ color, fontSize: 16 }} />,
  FileOutlined: (color: any) => <FileOutlined style={{ color, fontSize: 16 }} />,
};

// 图标名称映射（与 attachmentUtils.js 中的 FILE_CATEGORY_ICON_MAP 保持一致）
const CATEGORY_ICON_NAMES = {
  pdf: 'FilePdfOutlined',
  word: 'FileWordOutlined',
  excel: 'FileExcelOutlined',
  ppt: 'FilePptOutlined',
  image: 'FileImageOutlined',
  audio: 'SoundOutlined',
  video: 'PlayCircleOutlined',
  archive: 'FileZipOutlined',
  code: 'CodeOutlined',
  ebook: 'BookOutlined',
  data: 'DatabaseOutlined',
  text: 'FileTextOutlined',
  web: 'GlobalOutlined',
  default: 'FileOutlined',
};

/**
 * 根据文件扩展名渲染对应的图标组件
 *
 * @param {string} ext - 文件扩展名（小写），如 'pdf'、'docx'
 * @returns {React.ReactElement} 渲染后的图标组件
 */
function renderFileIcon(ext: any) {
  // 获取文件分类（如 'pdf'、'word'）
  const category = getFileCategory(ext);
  // 获取对应的主题颜色
  const color = getFileColor(category);
  // 获取图标组件名称
  const iconName = (CATEGORY_ICON_NAMES as any)[category as string] || CATEGORY_ICON_NAMES.default;
  // 从映射表中查找并渲染图标组件
  const renderFn = ICON_COMPONENT_MAP[iconName] || ICON_COMPONENT_MAP.FileOutlined;
  return renderFn(color);
}

/**
 * 解析状态到显示配置的映射表
 *
 * 用于主 PDF 附件行右侧的解析状态标签渲染。
 * 解析状态是论文级别的属性，只有主 PDF 才有解析状态（普通附件不涉及解析）。
 *
 * 四种状态：
 * - pending  → 灰色 "等待解析"：PDF 刚上传，后端尚未开始解析
 * - parsing  → 蓝色动画 "解析中..."：后端正在用 MinerU 提取文本
 * - done     → 绿色 "已解析"：解析完成（正常情况下会被 ENGINE_CONFIG 覆盖，显示具体引擎名）
 * - failed   → 红色 "解析失败"：解析过程中出现错误
 */
const PARSE_STATUS_CONFIG = {
  pending: { text: i18next.t('paperList:status.pending'), color: 'default' },    // 灰色标签：PDF 已上传但尚未开始解析
  parsing: { text: i18next.t('paperList:status.parsing'), color: 'processing' }, // 前端兼容键
  processing: { text: i18next.t('paperList:status.parsing'), color: 'processing' }, // 蓝色动画标签：后端正在解析（后端实际存储的值为 "processing"）
  done: { text: i18next.t('paperList:status.done'), color: 'success' },          // 绿色标签：解析完成（fallback，当 parse_engine 缺失时使用）
  failed: { text: i18next.t('paperList:status.failed'), color: 'error' },        // 红色标签：解析过程中出现异常，用户可点击重解析按钮重试
};

/**
 * 解析引擎显示配置映射表
 *
 * 当主 PDF 解析完成（parse_status === 'done'）时，不再显示通用的"已解析"标签，
 * 而是直接显示所使用的解析引擎名称（如 MinerU）。
 *
 * 设计意图：
 * - 一个标签同时传达两个信息：① 已解析完成（状态）② 用什么方式解析的（引擎）
 * - 替代之前表格中单独的"解析状态"列，减少信息冗余
 * - 不同引擎用不同颜色区分，方便用户快速识别
 *
 * 键名对应后端 paper.parse_engine 字段的值
 */
const ENGINE_CONFIG = {
  mineru: { color: 'blue', text: 'MinerU' },   // 蓝色标签：MinerU 引擎
};

// ========== 组件样式定义 ==========
// 使用内联样式对象，避免创建额外的 CSS 文件
const styles = {
  // 附件列表容器：浅灰背景，通过负 margin 覆盖文献行 td 的 padding 缝隙
  container: {
    padding: '0',
    backgroundColor: 'var(--bg-tertiary)',
    marginTop: '-4px',              // 向上延伸覆盖文献行的 padding-bottom
    marginBottom: '-4px',           // 向下延伸覆盖下一文献行的 padding-top
  },
  // 单个附件行：flex 布局，垂直居中
  row: {
    display: 'flex',                  // 弹性布局
    alignItems: 'center',             // 垂直居中
    padding: '4px 8px 4px 8px',     // 上右下左内边距，左侧与 antd 单元格 padding 对齐，图标落点与父文献标题左对齐
    borderBottom: '1px solid var(--border-color)', // 底部细线分隔
    minHeight: 32,                    // 最小行高 32px
    gap: 8,                           // 子元素间距 8px
    cursor: 'default',                // 默认鼠标样式
    transition: 'background-color 0.15s', // 背景色过渡动画
  },
  // 文件名列：flex 自适应宽度，文本溢出省略
  filename: {
    flex: 1,                          // 自适应填充剩余空间
    overflow: 'hidden',               // 隐藏溢出内容
    textOverflow: 'ellipsis',         // 溢出部分显示省略号
    whiteSpace: 'nowrap',             // 不换行
    fontSize: 13,                     // 字体大小 13px
    color: 'var(--text-primary)',                    // 主文字颜色
  },
  // 文件大小标签
  fileSize: {
    fontSize: 12,                     // 小号字体
    color: 'var(--text-tertiary)',                 // 辅助文字颜色
    flexShrink: 0 as const,                    // 不收缩
    minWidth: 60,                     // 最小宽度保证对齐
    textAlign: 'right' as const,               // 右对齐
  },
  // 操作按钮区域
  actions: {
    display: 'flex',                  // 弹性布局
    gap: 4,                           // 按钮间距 4px
    flexShrink: 0,                    // 不收缩
  },
  // 加载中容器
  loadingContainer: {
    padding: '16px 0 16px 16px',    // 左侧缩进 + 上下间距
    textAlign: 'center' as const,   // 居中
  },
};

/**
 * 附件列表主组件
 */
export default function AttachmentList({
  paper,          // 论文对象，包含 id、parse_status 等字段（用于读取主 PDF 的解析状态）
  attachments,    // 附件数组，每个附件包含 id、filename、file_type、file_size、is_primary 等
  loading,        // 是否正在加载附件列表
  onDelete,       // 删除回调：onDelete(attachmentId: number)
  onOpenPdf,      // 打开 PDF 回调：onOpenPdf(attachmentId: number)
  onSetPrimary,   // 设为主 PDF 回调：onSetPrimary(attachmentId: number) —— 将指定 PDF 附件切换为论文的主 PDF
}: any) {
  const { t } = useTranslation('paperList'); // i18n 翻译函数

  // ========== 加载中状态 ==========
  if (loading) {
    return (
      <div style={styles.container}>
        <div style={styles.loadingContainer}>
          {/* 加载动画 + 提示文字 */}
          <Spin indicator={<LoadingOutlined style={{ fontSize: 16 }} />} />
          <span style={{ marginLeft: 8, color: 'var(--text-tertiary)', fontSize: 13 }}>{t('attachment.loading')}</span>
        </div>
      </div>
    );
  }

  // 附件加载完仍为空：返回 null，避免展开行 <td> 自带 padding 在文献行下方撑出一条空白
  if (attachments.length === 0) {
    return null;
  }

  /**
   * 渲染单个附件行（统一渲染，主 PDF 和普通附件使用相同的行结构）
   *
   * 主 PDF 附件（is_primary=true）的特殊行为：
   * - 显示解析状态标签（从 paper.parse_status 读取，因为解析状态是论文级别的属性）
   * - 解析完成后文件名可点击打开阅读
   * - 解析完成前显示"打开阅读"按钮（禁用状态）
   *
   * 普通附件的行为：
   * - 显示文件大小
   * - PDF 附件可打开阅读
   * - 非 PDF 附件可下载
   * - 图片附件可预览
   *
   * @param {object} attachment - 附件对象
   * @param {number} index - 附件在列表中的序号（用于 key）
   */
  const renderAttachmentRow = (attachment: any, index: any) => {
    // 判断此附件是否为论文的主 PDF（上传论文时的 PDF）
    const isPrimary = attachment.is_primary;
    // 判断附件类型是否为 PDF（PDF 附件可以打开精读）
    const isPdf = attachment.file_type === 'pdf';
    // 判断附件类型是否为图片（图片附件支持预览）
    const isImage = ['jpg', 'jpeg', 'png', 'gif', 'bmp', 'svg', 'webp', 'tiff', 'tif'].includes(attachment.file_type);

    // 如果是主 PDF，从 paper 对象读取解析状态（解析状态存储在论文记录上，不在附件上）
    // parse_status 是论文级别的属性，因为解析操作是针对论文的主 PDF 进行的
    const parseStatus = isPrimary ? (paper.parse_status || 'pending') : null;

    // ========== 计算解析状态的标签显示配置（仅主 PDF 需要） ==========
    //
    // 核心逻辑：
    // - 如果是普通附件（非主 PDF），不需要解析状态标签，statusConfig 保持 null
    // - 如果是主 PDF 且已解析完成（parse_status === 'done'）且有 parse_engine 字段：
    //     使用 ENGINE_CONFIG 显示具体引擎名（如 "MinerU" 蓝色），
    //     一个标签同时传达"已解析"状态和"用什么引擎解析"两个信息
    // - 如果是主 PDF 但未解析完成（等待中/解析中/失败）：
    //     使用 PARSE_STATUS_CONFIG 显示对应的状态标签（如"等待解析"/"解析中..."/"解析失败"）
    // - 如果 parse_engine 字段缺失（如旧数据），fallback 到 PARSE_STATUS_CONFIG.done 显示通用"已解析"
    //
    let statusConfig = null; // 初始化为 null，普通附件不需要显示解析状态标签
    if (isPrimary) { // 仅主 PDF 需要计算解析状态标签配置
      if (parseStatus === 'done' && paper.parse_engine) {
        // 主 PDF 已解析完成且后端返回了 parse_engine 字段
        // 从 ENGINE_CONFIG 映射表中查找对应引擎的显示配置（颜色 + 文本）
        // 如果 parse_engine 值不在映射表中（如后端新增了引擎但前端尚未配置），回退到 MinerU 的配置
        const engine = (ENGINE_CONFIG as any)[paper.parse_engine] || ENGINE_CONFIG.mineru;
        // 用引擎的配置覆盖通用状态配置，使标签显示引擎名而非"已解析"
        statusConfig = { color: engine.color, text: engine.text };
      } else {
        // 其他状态（pending/parsing/failed）或 done 但缺少 parse_engine 字段
        // 从 PARSE_STATUS_CONFIG 映射表中获取对应状态的显示配置
        // 如果 parse_status 值异常（不在映射表中），回退到 pending 的配置
        statusConfig = (PARSE_STATUS_CONFIG as any)[parseStatus] || PARSE_STATUS_CONFIG.pending;
      }
    }

    // 判断主 PDF 是否已解析完成（解析完成后才能打开阅读）
    const isParseDone = isPrimary && parseStatus === 'done';

    // ========== 右键上下文菜单配置 ==========
    // 右键菜单选项：重解析（仅主 PDF）、设为主 PDF（非主 PDF）、删除（所有附件）
    const contextMenuItems = [
      // 非 primary 的 PDF 附件：显示"设为主 PDF"选项
      ...(!isPrimary && isPdf ? [{
        key: 'set-primary',
        label: t('attachment.setMainPdf'),
        icon: <StarOutlined />,
        onClick: () => { if (onSetPrimary) onSetPrimary(attachment.id); },
      }] : []),
      // 所有附件：显示"删除"选项
      {
        key: 'delete',
        label: t('common:delete'),
        icon: <DeleteOutlined />,
        danger: true,
        onClick: () => { onDelete(attachment.id); },
      },
    ];

    return (
      // Ant Design Dropdown 组件：包裹附件行，使其支持右键菜单
      // trigger={['contextMenu']} 表示仅在右键点击时弹出菜单，不影响普通的左键交互
      <Dropdown
        key={attachment.id || index}             // 使用附件 ID 作为 key，回退使用序号（保证列表渲染的唯一性）
        menu={{ items: contextMenuItems }}       // 传入菜单配置，items 数组定义菜单中的所有选项
        trigger={['contextMenu']}                // 触发方式：仅通过鼠标右键（contextMenu 事件）触发
      >
        <div
          style={{
            ...styles.row,
            // 第一行去掉顶部 padding，最后一行去掉底部 padding，
            // 避免在文献行与附件行之间产生可见缝隙
            paddingTop: index === 0 ? 0 : 4,
            paddingBottom: index === attachments.length - 1 ? 0 : 4,
          }}
          // 鼠标悬停效果：背景色变浅灰（深色模式适配）
          onMouseEnter={(e) => { e.currentTarget.style.backgroundColor = 'var(--bg-secondary)'; }}
          onMouseLeave={(e) => { e.currentTarget.style.backgroundColor = 'transparent'; }}
        >
        {/* 文件类型图标：根据扩展名渲染对应图标 */}
        {renderFileIcon(attachment.file_type)}

        {/* 文件名：PDF 附件可点击打开阅读 */}
        <span
          role={isPdf ? 'button' : undefined}
          tabIndex={isPdf ? 0 : undefined}
          style={{
            ...styles.filename,
            // 主 PDF 已解析完成时：蓝色可点击样式
            // 主 PDF 未解析完成时：普通灰色样式
            // 非 PDF 附件：普通灰色样式
            ...(isPdf ? {
              cursor: isPrimary ? (isParseDone ? 'pointer' : 'default') : 'pointer',
              color: isPrimary ? (isParseDone ? 'var(--color-primary)' : 'var(--text-primary)') : 'var(--color-primary)',
            } : {}),
          }}
          // 点击事件：主 PDF 需要解析完成才能打开，普通 PDF 直接打开
          // 所有 PDF 统一通过 attachmentId 导航，PaperReaderPage 通过 is_primary 判断行为
          onClick={() => {
            if (isPdf) {
              if (isPrimary) {
                // 主 PDF：仅解析完成后才能打开阅读
                if (parseStatus === 'done') onOpenPdf(attachment.id);
              } else {
                // 普通 PDF 附件：直接打开阅读
                onOpenPdf(attachment.id);
              }
            }
          }}
          // 键盘事件：支持回车键打开（无障碍访问）
          onKeyDown={(e) => {
            if (e.key === 'Enter' && isPdf) {
              if (isPrimary) {
                if (parseStatus === 'done') onOpenPdf(attachment.id);
              } else {
                onOpenPdf(attachment.id);
              }
            }
          }}
        >
          {/* 优先显示用户自定义标题，没有则显示原始文件名 */}
          {attachment.title || attachment.filename}
        </span>

        {/* 主 PDF：显示解析状态标签（从 paper 对象读取状态）。 */}
        {/* SSE 进度存在时附带百分比（如 "解析中... 42%"），stage 作悬停 tooltip */}
        {isPrimary && statusConfig && (
          <Tooltip title={paper.parse_stage || undefined} placement="top">
            <Tag
              color={statusConfig.color}
              // 解析完成显示对勾图标，解析中显示旋转加载图标
              // （'parsing' 是前端 SSE 乐观态，'processing' 是后端存储值——
              // 页面在解析中途刷新时列表行拿到的是后者）
              icon={
                parseStatus === 'done' ? <CheckCircleOutlined /> :
                parseStatus === 'parsing' || parseStatus === 'processing' ? <LoadingOutlined spin /> :
                null
              }
              style={{ margin: 0, fontSize: 11 }}
            >
              {paper.parse_progress != null && parseStatus !== 'done'
                ? `${statusConfig.text} ${paper.parse_progress}%`
                : statusConfig.text}
            </Tag>
          </Tooltip>
        )}

        {/* 非主 PDF 附件：显示文件大小 */}
        {!isPrimary && (
          <span style={styles.fileSize}>
            {formatFileSize(attachment.file_size)}
          </span>
        )}

        {/* 操作按钮区域：仅保留预览和下载（删除、重解析已移至右键菜单） */}
        {(isImage && !isPrimary || !isPdf) && (
        <div style={styles.actions}>
          {/* 图片附件：显示"预览"按钮（非主 PDF 的图片附件） */}
          {isImage && !isPrimary && (
            <Tooltip title={t('attachment.previewImage')}>
              <Button
                type="link"
                size="small"
                icon={<EyeOutlined />}
                onClick={() => window.open(getAttachmentUrl(attachment.id), '_blank')}
              />
            </Tooltip>
          )}

          {/* 非 PDF 附件显示下载按钮 */}
          {!isPdf && (
            <Tooltip title={t('attachment.downloadFile')}>
              <Button
                type="link"
                size="small"
                icon={<DownloadOutlined />}
                onClick={() => {
                  const url = getAttachmentUrl(attachment.id);
                  const a = document.createElement('a');
                  a.href = url;
                  a.download = attachment.filename;
                  document.body.appendChild(a);
                  a.click();
                  document.body.removeChild(a);
                }}
              />
            </Tooltip>
          )}
        </div>
        )}
        </div>
      </Dropdown>
    );
  };

  // ========== 主渲染 ==========
  return (
    <div style={styles.container}>
      {/* 附件列表：所有附件统一渲染，主 PDF（is_primary=true）排在最前 */}
      {attachments.map((att: any, idx: any) => renderAttachmentRow(att, idx))}
    </div>
  );
}
