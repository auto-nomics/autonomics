import { Toaster as Sonner, type ToasterProps } from "sonner";

/** Global toast host (shadcn/sonner). The app shell is always dark. */
const Toaster = ({ ...props }: ToasterProps) => (
  <Sonner
    className="toaster group"
    theme="dark"
    toastOptions={{
      classNames: {
        toast: "group toast group-[.toaster]:bg-card group-[.toaster]:text-foreground group-[.toaster]:border group-[.toaster]:shadow-lg",
        description: "group-[.toast]:text-muted-foreground",
      },
    }}
    {...props}
  />
);

export { Toaster };
