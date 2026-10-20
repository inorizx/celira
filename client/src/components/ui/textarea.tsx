import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"

/**
 * 多行输入。
 *
 * `bare` 那一档存在的原因（2026-10-20 主人真机："输入框看起来像被划分成两个区域"）：
 * 默认档（shadcn 原样）自带底、边、圆角与内距，**连 `dark:bg-input/30` 这块半透明底也在**。
 * 放进"容器本身就是一块表面"的地方（消息输入那颗胶囊）⇒ 框里还有一个框；草稿一长，那个
 * 内层浅色方块就显出来，看起来像把输入区分成了两半。
 * ⇒ `bare` 只剩**文字本身**：无底、无边、无内距、无阴影、无自己的焦点环 ——
 *   焦点环由**容器**给（`focus-within:border-ring`），不是被删掉。
 */
const textareaVariants = cva(
  "flex w-full transition-[color,box-shadow] outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50",
  {
    variants: {
      variant: {
        default:
          "field-sizing-content min-h-16 rounded-md border border-input bg-transparent px-3 py-2 text-base shadow-xs focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 md:text-sm dark:bg-input/30 dark:aria-invalid:ring-destructive/40",
        // ⚠ `field-sizing-fixed` 不是装饰：`field-sizing: content` 会**压过**显式 height，
        //   而消息输入框是自己量高度再写死的（判据在 `MessageInput` 那个 useEffect 里）。
        //   不锁成 fixed，删字数之后那块高度就缩不回去 —— 真机上表现为"一会儿高一会儿低"。
        bare: "field-sizing-fixed min-h-0 rounded-none border-0 bg-transparent p-0 text-body shadow-none focus-visible:ring-0 dark:bg-transparent",
      },
    },
    defaultVariants: { variant: "default" },
  },
)

function Textarea({
  className,
  variant,
  ...props
}: React.ComponentProps<"textarea"> & VariantProps<typeof textareaVariants>) {
  return (
    <textarea
      data-slot="textarea"
      data-variant={variant ?? "default"}
      className={cn(textareaVariants({ variant }), className)}
      {...props}
    />
  )
}

export { Textarea }
