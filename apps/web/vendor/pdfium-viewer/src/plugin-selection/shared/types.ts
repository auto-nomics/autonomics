import { SelectionMenuRenderFn, SelectionMenuPropsBase } from '../../utils/shared';

export interface SelectionSelectionContext {
  type: 'selection';
  pageIndex: number;
}

export type SelectionSelectionMenuRenderFn = SelectionMenuRenderFn<SelectionSelectionContext>;
export type SelectionSelectionMenuProps = SelectionMenuPropsBase<SelectionSelectionContext>;
