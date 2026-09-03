import { PdfDocumentObject, Rotation } from '../../models';
import { PageLayout } from '..';

export interface RenderPageProps extends PageLayout {
  rotation: Rotation;
  scale: number;
  document: PdfDocumentObject | null;
}
