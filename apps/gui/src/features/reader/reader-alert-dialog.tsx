import { createRoot, type Root } from "react-dom/client";
import { Dialog as Primitive } from "@base-ui/react/dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPortal, DialogOverlay, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from "@/components/ui/dialog";
import type { AlertDialogOptions } from "@/components/ui/alert-dialog";

/** React dialog bridge for the imperative reader views. */
export class ReaderAlertDialog {
  private host = document.createElement("div");
  private root: Root;
  private pending: ((confirmed: boolean) => void) | null = null;

  constructor(container: HTMLElement) {
    // Portaled dialogs must not create an extra row in the reader grid.
    this.host.style.display = "contents";
    container.append(this.host);
    this.root = createRoot(this.host);
  }

  open(options: AlertDialogOptions): Promise<boolean> {
    this.close(false);
    return new Promise(resolve => {
      this.pending = resolve;
      this.root.render(
        <Dialog open onOpenChange={(open) => { if (!open) this.close(false); }}>
          <DialogPortal>
            <DialogOverlay className="z-[230]" />
            <Primitive.Popup role="alertdialog" className="fixed top-1/2 left-1/2 z-[230] grid w-full max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 gap-4 rounded-xl bg-popover p-4 text-sm text-popover-foreground ring-1 ring-foreground/10 outline-none sm:max-w-sm">
              <DialogHeader>
                <DialogTitle>{options.title}</DialogTitle>
                {options.description && <DialogDescription>{options.description}</DialogDescription>}
              </DialogHeader>
              <DialogFooter>
                <Button variant="outline" onClick={() => this.close(false)}>{options.cancelLabel ?? "Cancel"}</Button>
                <Button variant={options.destructive ? "destructive" : "default"} onClick={() => this.close(true)}>{options.actionLabel ?? "OK"}</Button>
              </DialogFooter>
            </Primitive.Popup>
          </DialogPortal>
        </Dialog>,
      );
    });
  }

  private close(confirmed: boolean) {
    const resolve = this.pending;
    this.pending = null;
    if (resolve) { this.root.render(null); resolve(confirmed); }
  }

  destroy() {
    this.close(false);
    this.root.unmount();
    this.host.remove();
  }
}
