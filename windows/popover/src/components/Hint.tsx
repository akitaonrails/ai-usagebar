import type { ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface HintProps {
  align?: "center" | "end" | "start";
  children: ReactNode;
  content: ReactNode;
}

/**
 * The app's hover hint on any element, used instead of the native `title` (slow to appear,
 * unstyled, blind to the theme). With nothing to say it renders the child alone.
 */
export function Hint({ align = "center", children, content }: HintProps) {
  if (content === null || content === undefined || content === "") return children;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent align={align}>{content}</TooltipContent>
    </Tooltip>
  );
}
