import { useCallback, type ComponentProps } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { HoverCard, HoverCardContent, HoverCardTrigger } from './components/ui/hover-card';

type Rect = { left: number; top: number; width: number; height: number };
type State = { open: boolean; rect: Rect };
type ChangeDetails = Parameters<NonNullable<ComponentProps<typeof HoverCard>['onOpenChange']>>[1];

function EditorMount({ editor }: { editor: HTMLElement }) {
  const attach = useCallback((node: HTMLDivElement | null) => {
    if (node) node.append(editor);
  }, [editor]);
  return <div ref={attach} />;
}

function AnnotationCard({ state, editor, container, triggerId, onDismiss }: {
  state: State; editor: HTMLElement; container: HTMLElement; triggerId: string;
  onDismiss: () => void;
}) {
  // The actual highlights are PDF canvases/imperative DOM nodes. A virtual
  // anchor keeps their native text selection intact while Base UI positions
  // the real shadcn Hover Card, including reflow and collision handling.
  const anchor = {
    contextElement: container,
    getBoundingClientRect: () => DOMRect.fromRect({ x: state.rect.left, y: state.rect.top, width: state.rect.width, height: state.rect.height }),
  };
  const onOpenChange = (open: boolean, details: ChangeDetails) => {
    // Annotation editors are pinned by a click, rather than dismissed by a
    // hover timeout while the user is typing or moving between controls.
    if (!open && details.reason !== 'trigger-hover' && details.reason !== 'trigger-focus') onDismiss();
  };
  return (
    <HoverCard open={state.open} triggerId={triggerId} onOpenChange={onOpenChange}>
      <HoverCardTrigger
        id={triggerId}
        render={<span />}
        delay={100}
        closeDelay={200}
        aria-hidden="true"
        tabIndex={-1}
        className="reader-annotation-hover-trigger"
        style={{ left: state.rect.left, top: state.rect.top, width: state.rect.width, height: state.rect.height }}
      />
      <HoverCardContent
        role="dialog"
        aria-label="Annotation details"
        className="reader-annotation-popup p-0"
        positionerClassName="reader-annotation-positioner"
        portalContainer={container}
        positionMethod="fixed"
        anchor={anchor}
        side="bottom"
        sideOffset={12}
        align="center"
        alignOffset={0}
      >
        <EditorMount editor={editor} />
      </HoverCardContent>
    </HoverCard>
  );
}

let nextId = 0;

/** React boundary around the existing editor: drafts, save sequencing and
 * native event handlers remain on the same DOM node when the card repositions. */
export class AnnotationHoverCard {
  private host = document.createElement('div');
  private reactRoot: Root;
  private triggerId = `reader-annotation-trigger-${++nextId}`;
  private state: State = { open: false, rect: { left: 0, top: 0, width: 0, height: 0 } };

  constructor(private container: HTMLElement, private editor: HTMLElement, private onDismiss: () => void) {
    this.host.className = 'reader-annotation-hover-root';
    container.append(this.host);
    this.reactRoot = createRoot(this.host);
  }

  show(rect: Rect) {
    this.state = { open: true, rect };
    this.render();
  }

  hide() {
    if (!this.state.open) return;
    this.state = { ...this.state, open: false };
    this.render();
  }

  private render() {
    this.reactRoot.render(<AnnotationCard state={this.state} editor={this.editor} container={this.container} triggerId={this.triggerId} onDismiss={this.onDismiss} />);
  }

  destroy() { this.reactRoot.unmount(); this.host.remove(); }
}
