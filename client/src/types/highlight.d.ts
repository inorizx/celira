declare module 'highlight.js/lib/languages/*' {
  const languageFn: (hljs: unknown) => unknown
  export default languageFn
}
