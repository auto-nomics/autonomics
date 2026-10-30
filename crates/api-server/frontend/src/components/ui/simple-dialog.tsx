import { X } from "lucide-react";
import type { ReactNode } from "react";

/**
 * Minimal zero-dependency dialog (SiYuan-style centered panel over a
 * dimmed overlay). The workspace doesn't carry @radix-ui/react-dialog;
 * keep this tiny unless richer a11y becomes a requirement.
 */
export function SimpleDialog({
  open,
  onClose,
  title,
  description,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: string;
  children: ReactNode;
}) {
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 pt-[10vh]" onClick={onClose}>
      {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions */}
      <div
        role="dialog"
        aria-label={title}
        className="w-[92vw] max-w-lg rounded-lg border bg-background p-4 shadow-lg"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="mb-2 flex items-start justify-between gap-2">
          <div>
            <h2 className="text-base font-semibold">{title}</h2>
            {description ? (
              <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>
            ) : null}
          </div>
          <button
            type="button"
            className="rounded-md p-1 hover:bg-accent"
            onClick={onClose}
            aria-label="Close"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}
