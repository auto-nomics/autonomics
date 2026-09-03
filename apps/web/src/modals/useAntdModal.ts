import { useModal } from '@ebay/nice-modal-react';

/**
 * NiceModal + Ant Design Modal 集成 hook
 *
 * 将 nice-modal-react 的 useModal 映射为 antd Modal 所需的 open/onCancel 接口。
 * 用法：
 *   const { visible, close, modal } = useAntdModal();
 *   <Modal open={visible} onCancel={close} ...>
 */
export function useAntdModal() {
  const modal = useModal();
  return {
    visible: modal.visible,
    close: () => {
      modal.resolve(undefined);
      modal.hide();
    },
    modal,
  };
}
