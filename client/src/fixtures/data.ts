import type {
  AgentOwnership,
  BackendBinding,
  BackendCredential,
  Provider,
  ProviderModel,
} from './types'
import type { Attachment, Message, Participant, Room } from '../api/domain'
import { registerParticipantFallbackSource } from '../api/participantDirectory.ts'

export const SELF_ID = 'p-inori'

export const participants: Participant[] = [
  {
    id: SELF_ID,
    kind: 'human',
    displayName: 'Inori',
    initials: '我',
    signature: '把 AI 当邻居，不当工具。',
    since: '2026-08',
  },
  {
    id: 'p-aria',
    kind: 'human',
    displayName: 'Aria',
    initials: 'A',
    signature: '前端小样出来了吗？',
    since: '2026-08',
  },
  {
    id: 'a-courier',
    kind: 'agent',
    displayName: 'Courier',
    initials: 'C',
    signature: '消息契约与可靠协调的守门人。',
    since: '2026-08',
  },
  {
    id: 'a-guide',
    kind: 'agent',
    displayName: 'Celira 向导',
    initials: 'C',
    official: true,
    signature: '带你认识 Celira 的一切。',
    since: '平台内置',
  },
  {
    id: 'a-scribe',
    kind: 'agent',
    displayName: 'Scribe',
    initials: 'S',
    signature: '会议纪要与长文整理员。',
    since: '2026-08',
  },
  {
    id: 'p-vex',
    kind: 'human',
    displayName: '一个名字特别特别长应该被正确截断的参与者',
    initials: '长',
    signature: '签名也可能很长很长，长到必须被截断才能保证资料卡整齐的那种长度',
    since: '2026-07',
  },
]

// 把这份演示目录登记成参与者目录的「兜底来源」——只有兜底闸门开着（演示/开发路径）
// 时查询链才会用它。登记发生在**模块求值期**，早于任何 React 渲染（与 `store.ts`
// 在顶层注册闸门同款），演示通道首帧询问 `getParticipant` 时来源已就绪。
registerParticipantFallbackSource((id) => participants.find((p) => p.id === id))

export const rooms: Room[] = [
  {
    id: 'r-guide',
    name: 'Celira 向导',
    participantIds: [SELF_ID, 'a-guide'],
    lastMessagePreview: '欢迎来到 Celira，我带你认识这里',
    lastActivityAt: '12:50',
    unreadCount: 1,
    pinned: true,
  },
  {
    id: 'r-core',
    name: '世界讨论组',
    avatarLabel: '核',
    kind: 'group',
    participantIds: [SELF_ID, 'p-aria', 'a-courier'],
    lastMessagePreview: 'Aria：不着急，下午茶前回我就行。',
    lastActivityAt: '12:53',
    unreadCount: 2,
  },
  {
    id: 'r-courier',
    name: 'Courier',
    participantIds: [SELF_ID, 'a-courier'],
    lastMessagePreview: '直接聊，不用 @',
    lastActivityAt: '11:05',
    unreadCount: 0,
  },
  {
    id: 'r-courier-web',
    name: '网站项目',
    participantIds: [SELF_ID, 'a-courier'],
    lastMessagePreview: '落地页文案我改到第三版了',
    lastActivityAt: '10:26',
    unreadCount: 0,
  },
  {
    id: 'r-courier-rust',
    name: '学 Rust',
    participantIds: [SELF_ID, 'a-courier'],
    lastMessagePreview: '这份所有权笔记你先过一遍',
    lastActivityAt: '昨天',
    unreadCount: 2,
  },
  {
    id: 'r-aria',
    name: 'Aria',
    participantIds: [SELF_ID, 'p-aria'],
    lastMessagePreview: '前端小样出来了吗？',
    lastActivityAt: '昨天',
    unreadCount: 0,
  },
  {
    id: 'r-release',
    name: '开源发布筹备',
    avatarLabel: '开',
    participantIds: [SELF_ID, 'p-aria'],
    lastMessagePreview: 'README 还差愿景段',
    lastActivityAt: '周三',
    unreadCount: 3,
  },
  {
    id: 'r-long',
    name: '这个名字特别特别长的产品讨论小组应该被正确截断',
    avatarLabel: '长',
    participantIds: [SELF_ID, 'p-vex'],
    lastMessagePreview: '预览文本也可能很长很长，长到必须被截断才能保证列表整齐的那种长度',
    lastActivityAt: '周一',
    unreadCount: 128,
    muted: true,
  },
]

export const messages: Message[] = [
  {
    id: 'm-g0',
    roomId: 'r-guide',
    authorId: SELF_ID,
    body: '你开始了与 Celira 向导的会话',
    sentAt: '12:50',
    day: '今天',
    delivery: 'delivered',
    system: true,
  },
  {
    id: 'm-g1',
    roomId: 'r-guide',
    authorId: 'a-guide',
    body: '欢迎来到 Celira。这里 Human 和 Agent 都是平等的参与者，你可以在会话里直接 @我，也可以创建属于自己的 Agent。',
    sentAt: '12:50',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-g2',
    roomId: 'r-guide',
    authorId: 'a-guide',
    body: '试着去「我的 → 我接入的 Agent」给你的 Agent 挑一个模型，先从配置供应商开始。',
    sentAt: '12:50',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-00',
    roomId: 'r-core',
    authorId: 'p-aria',
    body: '昨天把三栏布局的初稿过了一遍，移动端还要再收一收。',
    sentAt: '18:30',
    day: '昨天',
    delivery: 'delivered',
  },
  {
    id: 'm-0',
    roomId: 'r-core',
    authorId: SELF_ID,
    body: 'Aria 加入了会话',
    sentAt: '12:35',
    day: '今天',
    delivery: 'delivered',
    system: true,
  },
  {
    id: 'm-0b',
    roomId: 'r-core',
    authorId: SELF_ID,
    body: 'Courier 已上线',
    sentAt: '12:36',
    day: '今天',
    delivery: 'delivered',
    system: true,
  },
  {
    id: 'm-1',
    roomId: 'r-core',
    authorId: 'a-courier',
    body: '消息契约 v0.3 我看完了。补一点：回复、明确沉默和超时在协议层是三种不同事件，UI 上建议也分开呈现，不要都折叠成"正在输入"。',
    sentAt: '12:38',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-2',
    roomId: 'r-core',
    authorId: 'p-aria',
    body: '同意。那"正在思考"和"已选择沉默"用不同的提示样式？',
    sentAt: '12:40',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-3',
    roomId: 'r-core',
    authorId: SELF_ID,
    body: '对，思考中用呼吸灯，沉默用一句静态文案。@Courier 沉默事件的 UI 语义你来定初稿？',
    sentAt: '12:42',
    day: '今天',
    delivery: 'delivered',
    mentions: ['a-courier'],
  },
  {
    id: 'm-6',
    roomId: 'r-core',
    authorId: 'a-courier',
    body: '可以，初稿要点：沉默用静态文案且不复现头像呼吸，超时给出明确重试入口，两者都不进入"正在输入"通道。',
    sentAt: '12:45',
    day: '今天',
    delivery: 'delivered',
    replyToId: 'm-3',
  },
  {
    id: 'm-4',
    roomId: 'r-core',
    authorId: SELF_ID,
    body: '这条消息还在发送中。',
    sentAt: '12:46',
    day: '今天',
    delivery: 'sending',
  },
  {
    id: 'm-5',
    roomId: 'r-core',
    authorId: SELF_ID,
    body: '这条消息发送失败了，需要给出明确的重试入口。',
    sentAt: '12:47',
    day: '今天',
    delivery: 'failed',
  },
  {
    id: 'm-7',
    roomId: 'r-core',
    authorId: 'p-aria',
    body: '首页主视觉先放这张概念图：',
    sentAt: '12:48',
    day: '今天',
    delivery: 'delivered',
    attachments: [{ id: 'att-1', kind: 'image', name: 'hero-concept.png', url: '/mock/img-1.svg' }],
  },
  {
    id: 'm-8',
    roomId: 'r-core',
    authorId: SELF_ID,
    body: '',
    sentAt: '12:48',
    day: '今天',
    delivery: 'delivered',
    attachments: [
      { id: 'att-2', kind: 'image', name: 'palette-dark.png', url: '/mock/img-2.svg' },
      { id: 'att-3', kind: 'image', name: 'palette-light.png', url: '/mock/img-3.svg' },
      { id: 'att-4', kind: 'image', name: 'bubble-study.png', url: '/mock/img-4.svg' },
      { id: 'att-5', kind: 'image', name: 'presence-states.png', url: '/mock/img-5.svg' },
    ],
  },
  {
    id: 'm-9',
    roomId: 'r-core',
    authorId: 'a-courier',
    body: '契约草案同步一份，改动点都在第三节。',
    sentAt: '12:49',
    day: '今天',
    delivery: 'delivered',
    attachments: [{ id: 'att-6', kind: 'file', name: '消息契约-v0.3-draft.md', size: '18.2 KB' }],
  },
  {
    id: 'm-10',
    roomId: 'r-core',
    authorId: 'a-courier',
    body: '（这条草稿串先撤回，稍后整理进第三节）',
    sentAt: '12:49',
    day: '今天',
    delivery: 'delivered',
    recalledAt: '12:49',
  },
  {
    id: 'm-11',
    roomId: 'r-core',
    authorId: 'p-aria',
    body: '@Inori 契约草案第三节我加了两条边界用例，你看一眼？',
    sentAt: '12:52',
    day: '今天',
    delivery: 'delivered',
    mentions: [SELF_ID],
  },
  {
    id: 'm-12',
    roomId: 'r-core',
    authorId: 'p-aria',
    body: '不着急，下午茶前回我就行。',
    sentAt: '12:53',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-hw1',
    roomId: 'r-courier-web',
    authorId: SELF_ID,
    body: '这个房间专门聊官网重做，契约讨论还在原来那间。',
    sentAt: '10:12',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-hw2',
    roomId: 'r-courier-web',
    authorId: 'a-courier',
    body: '明白，这边我只跟进网站项目。落地页结构我先给一版？',
    sentAt: '10:15',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-hw3',
    roomId: 'r-courier-web',
    authorId: SELF_ID,
    body: '给，先出文案再排版。',
    sentAt: '10:20',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-hw4',
    roomId: 'r-courier-web',
    authorId: 'a-courier',
    body: '落地页文案我改到第三版了，主标题候选：「让 AI 成为邻居」。',
    sentAt: '10:26',
    day: '今天',
    delivery: 'delivered',
  },
  {
    id: 'm-hr1',
    roomId: 'r-courier-rust',
    authorId: SELF_ID,
    body: '开个头：所有权这一章我总卡壳。',
    sentAt: '21:02',
    day: '昨天',
    delivery: 'delivered',
  },
  {
    id: 'm-hr2',
    roomId: 'r-courier-rust',
    authorId: 'a-courier',
    body: '卡住很正常。记住一句话：每个值同一时刻只有一个所有者。\n\n```rust\nfn main() {\n    let s1 = String::from("hello");\n    let s2 = s1; // s1 在这里移动，之后失效\n    println!("{s2}");\n}\n```',
    sentAt: '21:05',
    day: '昨天',
    delivery: 'delivered',
  },
  {
    id: 'm-hr3',
    roomId: 'r-courier-rust',
    authorId: 'a-courier',
    body: '这份所有权笔记你先过一遍，明天我们做借用练习。',
    sentAt: '21:06',
    day: '昨天',
    delivery: 'delivered',
  },
]

export const demoAttachments: { image: Attachment[]; file: Attachment[] } = {
  image: [{ id: 'att-demo-image', kind: 'image', name: 'concept-share.png', url: '/mock/img-1.svg' }],
  file: [{ id: 'att-demo-file', kind: 'file', name: 'Celira-产品介绍.pdf', size: '2.4 MB' }],
}

export const agentOwnerships: AgentOwnership[] = [
  { agentId: 'a-courier', ownerId: SELF_ID, createdAt: '2026-08-02' },
  { agentId: 'a-scribe', ownerId: SELF_ID, createdAt: '2026-08-11' },
]

export const backendBindings: BackendBinding[] = [
  {
    agentId: 'a-courier',
    backendLabel: 'Courier Backend',
    state: 'connected',
    scopes: ['收发消息', '读取会话元数据'],
  },
  {
    agentId: 'a-scribe',
    backendLabel: 'Scribe Backend',
    state: 'offline',
    scopes: ['收发消息'],
  },
]

export const backendCredentials: BackendCredential[] = [
  { id: 'cred-1', agentId: 'a-courier', label: '主力凭证 · 本设备签发', state: 'active', issuedAt: '2026-08-02' },
  { id: 'cred-2', agentId: 'a-courier', label: '旧笔记本凭证', state: 'revoked', issuedAt: '2026-08-05' },
  { id: 'cred-3', agentId: 'a-scribe', label: '主力凭证 · 本设备签发', state: 'active', issuedAt: '2026-08-11' },
]

export const myAgentIds = agentOwnerships.filter((o) => o.ownerId === SELF_ID).map((o) => o.agentId)

export const providers: Provider[] = [
  {
    id: 'pv-openai',
    name: 'OpenAI',
    initials: 'O',
    apiAddress: 'https://api.openai.com/v1',
    status: 'connected',
    keyUrl: 'https://platform.openai.com/api-keys',
  },
  {
    id: 'pv-deepseek',
    name: 'DeepSeek',
    initials: 'D',
    apiAddress: 'https://api.deepseek.com/v1',
    status: 'connected',
    keyUrl: 'https://platform.deepseek.com/api_keys',
  },
  {
    id: 'pv-zhipu',
    name: '智谱开放平台',
    initials: '智',
    apiAddress: 'https://open.bigmodel.cn/api/paas/v4',
    status: 'unconfigured',
  },
  {
    id: 'pv-custom',
    name: '自定义（OpenAI 兼容）',
    initials: '自',
    apiAddress: '',
    status: 'unconfigured',
  },
]

export const providerModels: ProviderModel[] = [
  { id: 'md-gpt5', providerId: 'pv-openai', family: 'gpt', label: 'gpt-5', capabilities: ['text', 'vision', 'tools'] },
  { id: 'md-gpt5-mini', providerId: 'pv-openai', family: 'gpt', label: 'gpt-5-mini', capabilities: ['text', 'tools'] },
  { id: 'md-gpt5-nano', providerId: 'pv-openai', family: 'gpt', label: 'gpt-5-nano', capabilities: ['text'] },
  { id: 'md-o4', providerId: 'pv-openai', family: 'o 系列', label: 'o4', capabilities: ['text', 'tools'] },
  { id: 'md-o4-mini', providerId: 'pv-openai', family: 'o 系列', label: 'o4-mini', capabilities: ['text', 'tools'] },
  { id: 'md-ds-v4-pro', providerId: 'pv-deepseek', family: 'deepseek', label: 'deepseek-v4-pro', capabilities: ['text', 'tools'] },
  { id: 'md-ds-v4-flash', providerId: 'pv-deepseek', family: 'deepseek', label: 'deepseek-v4-flash', capabilities: ['text'] },
  { id: 'md-ds-r2', providerId: 'pv-deepseek', family: 'reasoner', label: 'deepseek-r2', capabilities: ['text', 'tools'] },
  { id: 'md-ds-r2-lite', providerId: 'pv-deepseek', family: 'reasoner', label: 'deepseek-r2-lite', capabilities: ['text'] },
  { id: 'md-glm5', providerId: 'pv-zhipu', family: 'glm', label: 'glm-5', capabilities: ['text', 'vision', 'tools'] },
  { id: 'md-glm5-air', providerId: 'pv-zhipu', family: 'glm', label: 'glm-5-air', capabilities: ['text', 'tools'] },
  { id: 'md-glm5v', providerId: 'pv-zhipu', family: 'glm-v', label: 'glm-5v', capabilities: ['text', 'vision'] },
  { id: 'md-custom', providerId: 'pv-custom', family: 'custom', label: '自定义模型', capabilities: ['text'] },
]

export const myAgentConfig = {
  agentId: 'a-courier',
  backendLabel: 'Courier Backend · 已授权',
  modelId: 'md-ds-v4-flash',
}

export function getProvider(id: string): Provider | undefined {
  return providers.find((p) => p.id === id)
}

export function getModel(id: string): ProviderModel | undefined {
  return providerModels.find((m) => m.id === id)
}
