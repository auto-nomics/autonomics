/**
 * 模态框注册中心
 *
 * 所有通过 NiceModal 管理的模态框在此统一注册。
 * 在 App.tsx 中 import './modals' 触发注册。
 */
import NiceModal from '@ebay/nice-modal-react';

// 独立模态框组件
import ImageLightboxModal from '../features/ai-chat/components/ImageLightboxModal';
import ConversationHistoryModal from '../components/ConversationHistoryModal';
import ModelConfigModal from '../features/ai-chat/components/ModelConfigModal';
import DuplicateResolutionModal from '../components/DuplicateResolutionModal';
import TemplateManagerModal from '../components/TemplateManagerModal';
import CitationBatchExportModal from '../components/CitationBatchExportModal';

NiceModal.register('image-lightbox', ImageLightboxModal);
NiceModal.register('conversation-history', ConversationHistoryModal);
NiceModal.register('model-config', ModelConfigModal);
NiceModal.register('duplicate-resolution', DuplicateResolutionModal);
NiceModal.register('template-manager', TemplateManagerModal);
NiceModal.register('citation-batch-export', CitationBatchExportModal);
