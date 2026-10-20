//! im-social：Celira IM 的**关系面**（社交包）——看关系账与黑名单、发起申请、回答申请、拉黑与
//! 取消拉黑、结束一段关系。**七条**（两个只读 ＋ 五个写，每个写各配一项只读复核）。
//!
//! 它是 2026-10-02 从 `im` 里拆出来的那一组动词：动作 id、参数表、回执文案、失败分类、
//! wire 字段与幂等键规则**一字不改**，只是换了拥有者。它**不预装**：装配了，但要主体
//! 展开那一下才上场（与 files / timer 同款）。
//!
//! **为什么这一半必须跟关系一起放开**：关系是主体之间的事实，而一项权利若在**执行端够不着**，
//! 就等于没有（「角色不可达 = 没有」「事实够不着 = 没有」——同一条纪律的第三次）。所以"能加"
//! 与"能拒、能撤"必须同时在场：只给一半，主体就只看得见一段自己动不了的关系。
//!
//! 共用的通道机制在 [`crate::im_channel`]（App 之间不得有实现依赖）。
//!
//! **它刻意不含的那几格（是边界，不是漏补）**：
//!
//! - **看对端的资料卡**：世界的资料读面是**房内**作用域——那一条读法要求对那间房有 active
//!   成员周期（`GET /v1/backend/rooms/{room_id}/members/{participant_id}/profile`），
//!   而**关系不是可见性的依据**（关系 ≠ 可见）。⇒ 关系成立但不同房时，本包只给得出关系账里
//!   那一行名字（`im_contacts` 的 `name`）；要看得更全，**开一间 1:1 的房把对端拉进来**。
//!   本包**不**补一条"按关系作用域读资料"的动词：那等于给 Agent 一条 Human 没有的路
//!   （裁决见 `agent/README.md` §3.89）。
//! - **按名字找人**：本包没有"搜索 / 找陌生人"的动词——世界刻意**不给存在性 oracle**（"不存在 /
//!   非 active / 被对端拉黑"合成同一个 404）。⇒ 加人之前得先有一个对端的标识，它来自已经看见的
//!   那一格（`im_room_members` 的成员清单、`im_contacts` 的 `peer` 都是这样的来源）；本包不提供
//!   "试一个名字看看在不在"的入口。
//!
//! ⚠ **`im_end_contact` 那四条契约语义是给主体读的判据，不是备注**：单方面生效 · 不通知对端 ·
//! 旧房永久封存 · 它不是拉黑——它们决定"要不要按下去"，**必须**留在 `with_details` 里；删短它
//! 等于让主体在不了解后果时动手。

use crate::clock::localize_rfc3339;
use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};
use crate::im_channel::{ImAccess, ImChannel, ImFailure, MAX_ID_BYTES, Reply, idempotency_key};

/// 这个 App 对外的稳定标识（展开动作的目标就是它）。
pub const IM_SOCIAL_APP_ID: &str = "im-social";

/// **看关系**动词（只读面，批次三第三刀 第②步）：我的联系人关系，以及**哪一条在
/// 等我回答**（`direction = incoming`）。
pub const CAPABILITY_CONTACTS: &str = "im_contacts";
/// **看拉黑**动词（只读面）：我拉黑了谁（拉黑的复核面）。
pub const CAPABILITY_BLOCKS: &str = "im_blocks";
/// **发起关系申请**动词（写面）：我主动想和某个主体建立关系（对方要自己接受）。
pub const CAPABILITY_CONTACTS_REQUEST: &str = "im_contacts_request";
/// **回答关系申请**动词（写面）：接受或拒绝**别人给我的**那条申请。
pub const CAPABILITY_ANSWER_CONTACT: &str = "im_answer_contact";
/// **拉黑**动词（写面）：不再接受某个主体的关系申请。
pub const CAPABILITY_BLOCK: &str = "im_block";
/// **解除拉黑**动词（写面）：重新允许某个主体向我发起申请。
pub const CAPABILITY_UNBLOCK: &str = "im_unblock";

/// **解除一段关系**（2026-10-13）：标准 IM 里"加好友"的另一半——能加就得能删。
/// 走 Agent 侧那条**既有**端点（`DELETE /v1/backend/contacts/{participant_id}`，
/// 批次三第二刀那五条关系端点之一；当年那批同样写着「不动执行端」）。
/// ⚠ **端点早就在 ⇒ 这条动作不解封任何契约、也不改 im 服务端**：缺的只是这一侧的那个动词。
pub const CAPABILITY_END_CONTACT: &str = "im_end_contact";

/// 关系面的**对端主体**参数名（取自 `im_contacts`；拉黑/解除也用它）。
pub const PARAM_PARTICIPANT: &str = "participant";
/// 回答关系申请时的**决定**参数名：`accept` | `decline`。
pub const PARAM_DECISION: &str = "decision";

/// 关系面路径（读列表；批次三第二刀的关系面 + 第三刀第②步的同意面）。
const CONTACTS_PATH: &str = "/v1/backend/contacts";
/// 发起关系申请的路径（写；必带幂等键）。
const CONTACT_REQUESTS_PATH: &str = "/v1/backend/contacts/requests";
/// 回答关系申请的路径后缀（写：`accept` | `decline`）。
const ACCEPT_SUFFIX: &str = "/accept";
const DECLINE_SUFFIX: &str = "/decline";
/// 拉黑面路径（读列表 + 写：拉黑 / 解除）。
const BLOCKS_PATH: &str = "/v1/backend/blocks";

/// 关系面（社交包）：不预装，展开才上场。
pub struct ImSocialApp {
    channel: ImChannel,
}

impl ImSocialApp {
    /// 装配一个 im-social app（请求客户端在这里建一次，含单次请求的墙钟上限）。
    pub fn new(access: ImAccess) -> Option<Self> {
        Some(Self {
            channel: ImChannel::new(access)?,
        })
    }

    /// 本 App 声明的全部能力（顺序固定）：两个只读（关系账 / 黑名单）与四个写（发起申请、回答、
    /// 拉黑、解除；各配一项只读复核——见 `declared_effects`）。**没被声明的动作是只读**
    /// （不声明 = 只读，默认方向通向更少的能力）。
    fn own_capabilities() -> Capabilities {
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_CONTACTS),
                "看我的关系：我和谁有关系、谁在等我回答（`direction = incoming` 的那些）",
                vec![],
            )
            .with_details(
                "它是什么：你与世界之间的关系账——每一条是一个对端（Human 或 Agent）\
                 加上这段关系的状态。\n\
                 方向是给你的：`direction = incoming` 表示对方向你发起、只有你能接受或拒绝；\
                 `outgoing` 表示是你发起的、在等对方。回答之前先看它，不要靠试错。\n\
                 状态：`pending`（还没定）/ `accepted`（成立了）。闭合的关系（被拒绝、被解除、\
                 被拉黑结束的）不在这里——结束了就是结束了，账上不留半条。\n\
                 它是只读的：不改任何东西。要回答用 `im_answer_contact`，\
                 要主动加人用 `im_contacts_request`。\n\
                 回执形状：`contacts = N, incoming = M` 开头，逐条 `peer`（身份）/`kind`/\
                 `name`/`status`/`direction`。\n\
                 失败：读不到（连不上、凭据不被接受）⇒ 各自的稳定分类；不会给你一个假的空表。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_BLOCKS),
                "看我拉黑了谁（我不再接受谁的关系申请）",
                vec![],
            )
            .with_details(
                "它是什么：你拉黑的名单。拉黑是关系的对称面——能加、能拒绝、能解除，\
                 就能拉黑；它只拦新关系的入口，不影响别的一切。\n\
                 它不拦消息：被拉黑的人若已经在某间房里，照旧能在那间房说话（拉黑不进消息\
                 主链）；你不想听，用退房，不是用拉黑。\n\
                 对方不会知道：被拉黑的人看不到任何黑名单语义——他向你申请时只会得到\
                 「找不到这位主体」，与你根本不存在时一模一样（世界刻意不给存在性 oracle）。\n\
                 回执形状：`blocks = N` 开头，逐条 `peer`/`kind`/`name`/`since`。\n\
                 它是只读的：要拉黑用 `im_block`，要解除用 `im_unblock`。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_CONTACTS_REQUEST),
                "主动向一个主体发起关系申请——想和他建立关系（要不要答应是他自己的事）",
                vec![ParamSpec::new(
                    PARAM_PARTICIPANT,
                    true,
                    "对端主体标识（Human 或 Agent 都可以）",
                )],
            )
            .with_details(
                "你不是在通知他，你是在问他：申请落在对方那里等他回答（`pending`、\
                 他那一侧看到 `direction = incoming`）。他答不答是他的事，你这里只是等。\n\
                 一对之间只有一段关系：你们之间已经有关系（pending 或 accepted）时，\
                 再申请一次不会开出第二条——直接回到那一段（回执里 `replayed = yes`）。\
                 所以重复申请不会造成两段关系。\n\
                 被拉黑时你什么也看不出来：对方拉黑了你，你得到的会是\
                 `unknown-participant`——与他根本不存在时一模一样（世界刻意不给存在性\
                 oracle，这是不泄露，不是出错）。\n\
                 回执形状：`peer`/`kind`/`name`/`relation`（`pending`）/`replayed`。\n\
                 失败：目标不可用（不存在 / 非 active / 被对方拉黑）⇒ `unknown-participant`；\
                 向自己申请 ⇒ `invalid-argument`；同一个幂等键配了另一个目标 ⇒ `idempotency-conflict`。\n\
                 只读复核：`im_contacts`——那一段在不在、是什么状态，就是答案。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_ANSWER_CONTACT),
                "回答一条给我的关系申请：接受（`accept`）或拒绝（`decline`）",
                vec![
                    ParamSpec::new(
                        PARAM_PARTICIPANT,
                        true,
                        "对端主体标识（取自 im_contacts 里 direction = incoming 的那条）",
                    ),
                    ParamSpec::new(PARAM_DECISION, true, "`accept` 或 `decline`"),
                ],
            )
            .with_details(
                "这是你的同意：被拉进一段关系需要你自己答应——你的 Owner 不能代你答应\
                 （它保留的是撤销你、以及对外授权的那些决定，不是替你点头）。\n\
                 只有你能回答给你的那条：对方向你发起的（`incoming`）才轮到你；回答一条\
                 你发起的、或一对之间根本没有关系的，一律 `unknown-contact`——世界不告诉你\
                 是哪一种（那会泄露别人的关系状态）。先 `im_contacts` 看清 direction。\n\
                 接受之后：那段关系成为 `accepted`，对方从此可以邀请你进群、可以向你开\
                 1:1 的房（关系就是准入）。\n\
                 拒绝之后：那段关系以 `declined` 闭合、不占位——对方可以重新发起\
                 （新的一次申请要用新的幂等键，那是他那侧的事）。\n\
                 回执形状：`peer`/`decision`/`relation`（`accepted` 或 `declined`）。\n\
                 失败：不是给你的 / 没有这段关系 ⇒ `unknown-contact`；\
                 对端主体不可用 ⇒ `unknown-participant`；参数非法 ⇒ `invalid-argument`（不出网）。\n\
                 只读复核：`im_contacts`——那一条现在是什么状态、还在不在，就是答案。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_BLOCK),
                "拉黑一个主体：我不再接受他的关系申请（只拦入口，不拦消息）",
                vec![ParamSpec::new(
                    PARAM_PARTICIPANT,
                    true,
                    "要拉黑的主体标识（Human 或 Agent 都可以）",
                )],
            )
            .with_details(
                "它会先结束你们之间既有的关系段（同一事务）：拉黑 ⊇ 解除——不先结束，\
                 拉黑一个已经是你联系人的人就毫无切断力。\n\
                 它不拦消息：被拉黑的人若已经在某间房里，照旧能在那间房说话；拉黑不进\
                 消息主链。\n\
                 对方不会知道：他之后向你申请只会得到「找不到这位主体」，与你根本不存在时\
                 一模一样。\n\
                 幂等：已经拉黑过 ⇒ 成功、什么也没多发生（`blocked = yes`）。\n\
                 回执形状：`peer`/`blocked`。\n\
                 失败：拉黑自己 ⇒ `invalid-argument`；目标不可用 ⇒ `unknown-participant`。\n\
                 只读复核：`im_blocks`——他在不在那份名单里，就是答案。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_END_CONTACT),
                "解除与一个主体的关系（删好友）——单方面生效，不用对方同意",
                vec![ParamSpec::new(
                    PARAM_PARTICIPANT,
                    true,
                    "对端主体标识（取自 im_contacts）",
                )],
            )
            .with_details(
                "单方面即时生效：不需要对方同意，也不通知对方（世界不替他记这件事）。\n\
                 旧房永久封存：你们之间那间 1:1 的房从此不再可用（关系是准入）。\n\
                 它不是拉黑：拉黑是另一条动作（`im_block`），而且拉黑 ⊇ 解除——\
                 已经拉黑的人，解除这一段不会解除拉黑。要彻底断掉用 `im_block`。\n\
                 幂等：你们之间本来就没有 active 段 ⇒ 成功（零新事实，不报错）。\n\
                 回执形状：`peer`、`ended`。\n\
                 只读复核：`im_contacts`——那一段还在不在、是什么状态，就是答案。\n\
                 失败：对端主体不可用（不存在 / 非 active）⇒ `unknown-participant`；\
                 参数非法 ⇒ `invalid-argument`（不出网）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_UNBLOCK),
                "解除拉黑：重新允许这个主体向我发起关系申请",
                vec![ParamSpec::new(
                    PARAM_PARTICIPANT,
                    true,
                    "要解除的主体标识（取自 im_blocks）",
                )],
            )
            .with_details(
                "它只放开入口：解除之后对方可以重新申请——但关系不会自动恢复\
                 （那一段已经闭合了，要重新建立得他再发起、你再接受）。\n\
                 幂等：没拉黑过 ⇒ 成功（`unblocked = yes`），不报错。\n\
                 回执形状：`peer`/`unblocked`。\n\
                 失败：目标不可用 ⇒ `unknown-participant`。\n\
                 只读复核：`im_blocks`——他还在不在那份名单里，就是答案。",
            ),
        ])
    }

    /// 关系面：`GET /v1/backend/contacts`（批次三第二刀的关系面 + 第三刀第②步的同意面）。
    ///
    /// 无参数。回执按 `direction` 如实呈现——**哪一条在等我回答**是主体做决定所需的信息，
    /// 不该靠试错拿（`accept` 一条不是给自己的会得到 `unknown-contact`）。
    fn contacts(&self, _params: &ActionParams) -> ActionOutcome {
        match self.channel.exchange("GET", CONTACTS_PATH, None, None) {
            Reply::Answered(value) => match render_contacts(&value) {
                Ok(text) => {
                    ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_CONTACTS, text)])
                }
                Err(failure) => ActionOutcome::failure(failure.as_str()),
            },
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 拉黑名单：`GET /v1/backend/blocks`。
    fn blocks(&self, _params: &ActionParams) -> ActionOutcome {
        match self.channel.exchange("GET", BLOCKS_PATH, None, None) {
            Reply::Answered(value) => match render_blocks(&value) {
                Ok(text) => {
                    ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_BLOCKS, text)])
                }
                Err(failure) => ActionOutcome::failure(failure.as_str()),
            },
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 发起关系申请：`POST /v1/backend/contacts/requests`（必带幂等键）。
    fn contacts_request(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let Some(participant) = valid_participant(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        // **体里的字段名是 wire 名**（`participant_id`），不是我们的参数名（`participant`）
        // ——与世界契约 `CreateContactRequest` 逐字对齐（未知字段一律 400）。
        let payload = serde_json::json!({ "participant_id": participant }).to_string();
        let key = idempotency_key(operation);
        match self
            .channel
            .exchange("POST", CONTACT_REQUESTS_PATH, Some(payload), Some(&key))
        {
            Reply::Answered(value) => {
                let Some(peer) = render_peer(&value, "contact") else {
                    return ActionOutcome::unknown();
                };
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_CONTACTS_REQUEST,
                    render_contact_requested(&peer),
                )])
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 契约把「不存在 / 非 active / 被对方拉黑」合成同一个 404（不泄露）。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownParticipant.as_str()),
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::IdempotencyConflict.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 回答给我的那条申请：`POST /v1/backend/contacts/{peer}/accept|decline`。
    ///
    /// **这是主体的同意**：只有被申请的那一方能答；答一条不是给自己的得到
    /// `unknown-contact`（世界不说是哪一种——那会泄露别人的关系状态）。
    fn answer_contact(&self, params: &ActionParams) -> ActionOutcome {
        let Some(participant) = valid_participant(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(decision) = valid_decision(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let suffix = if decision == DECISION_ACCEPT {
            ACCEPT_SUFFIX
        } else {
            DECLINE_SUFFIX
        };
        let path = format!("{CONTACTS_PATH}/{participant}{suffix}");
        // 无体、无键：回答是"一次决定"，第二次要么已是那个状态（accept 幂等），
        // 要么那段已经不是 pending（decline ⇒ unknown-contact）。
        match self.channel.exchange("POST", &path, None, None) {
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_ANSWER_CONTACT,
                render_answered(&value, participant, decision),
            )]),
            // decline 成功是 204（空体）——那也是"这次回答已被接受"。
            Reply::Unreadable => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_ANSWER_CONTACT,
                render_answered_declined(participant),
            )]),
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownContact.as_str()),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// 拉黑：`POST /v1/backend/blocks/{peer}`（幂等 204）。
    fn block(&self, params: &ActionParams) -> ActionOutcome {
        let Some(participant) = valid_participant(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let path = format!("{BLOCKS_PATH}/{participant}");
        match self.channel.exchange("POST", &path, None, None) {
            Reply::Answered(_) | Reply::Unreadable => {
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_BLOCK,
                    render_blocked(participant),
                )])
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownParticipant.as_str()),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// 解除一段关系：`DELETE /v1/backend/contacts/{peer}`（幂等 204——没有 active 段也是成功）。
    ///
    /// 契约（批次三第二刀）：**单方面即时生效**、**不通知对端**、旧房**永久封存**。
    /// 它不是拉黑——**拉黑 ⊇ 解除**，两条各自独立（与 `im_block` 同一条口径）。
    /// **同参数第二次是成功 no-op**（幂等 204、零新事实）⇒ 这一格判 `repeatable-write`；
    /// 复核是**关系账**（那一段还在不在、是什么状态），不是黑名单。
    fn end_contact(&self, params: &ActionParams) -> ActionOutcome {
        let Some(participant) = valid_participant(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let path = format!("{CONTACTS_PATH}/{participant}");
        match self.channel.exchange("DELETE", &path, None, None) {
            Reply::Answered(_) | Reply::Unreadable => {
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_END_CONTACT,
                    render_ended(participant),
                )])
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 目标不可用（不存在 / 非 active）⇒ 与拉黑同一条口径。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownParticipant.as_str()),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// 解除拉黑：`DELETE /v1/backend/blocks/{peer}`（幂等 204）。
    fn unblock(&self, params: &ActionParams) -> ActionOutcome {
        let Some(participant) = valid_participant(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let path = format!("{BLOCKS_PATH}/{participant}");
        match self.channel.exchange("DELETE", &path, None, None) {
            Reply::Answered(_) | Reply::Unreadable => {
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_UNBLOCK,
                    render_unblocked(participant),
                )])
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownParticipant.as_str()),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }
}

impl ActionPort for ImSocialApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        Self::own_capabilities()
    }

    /// 局部准入：本包的六项一律放行；别的动词不属于这个 App。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_CONTACTS
            | CAPABILITY_BLOCKS
            | CAPABILITY_CONTACTS_REQUEST
            | CAPABILITY_ANSWER_CONTACT
            | CAPABILITY_BLOCK
            | CAPABILITY_UNBLOCK
            | CAPABILITY_END_CONTACT => ActionAdmission::Admitted,
            _ => ActionAdmission::refused(crate::core::UNKNOWN_CAPABILITY),
        }
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match action.id().as_str() {
            // 关系面（只读）：我的关系账与拉黑名单——`direction` 告诉我哪条在等我回答。
            CAPABILITY_CONTACTS => self.contacts(action.params()),
            CAPABILITY_BLOCKS => self.blocks(action.params()),
            // 写：**带键**（发起申请要幂等：同一个 operation 重放不产生第二条关系）。
            CAPABILITY_CONTACTS_REQUEST => self.contacts_request(operation, action.params()),
            // 写：**不带键**（回答是一次决定；accept 幂等、decline 之后那段已闭合）。
            CAPABILITY_ANSWER_CONTACT => self.answer_contact(action.params()),
            // 写：**不带键**（拉黑/解除都是幂等 204，见各自的详情卡）。
            CAPABILITY_BLOCK => self.block(action.params()),
            CAPABILITY_UNBLOCK => self.unblock(action.params()),
            // 解除关系（2026-10-13）：**不带键**（幂等由"有没有 active 段"承担，见 `end_contact`）。
            CAPABILITY_END_CONTACT => self.end_contact(action.params()),
            _ => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
        }
    }
}

impl crate::app_contract::AppPort for ImSocialApp {
    fn app_id(&self) -> &str {
        IM_SOCIAL_APP_ID
    }

    fn description(&self) -> &str {
        "Celira IM 的关系面：看我的联系人与拉黑名单，可以主动申请加人、回答别人给我的申请、拉黑与解除（只碰 im 自己的关系账与黑名单，不能改任何别的东西）"
    }

    fn declared_capabilities(&self) -> Capabilities {
        // 声明必须与实例的真实能力一致：OS 的归属索引就是按这份声明建的。
        Self::own_capabilities()
    }

    /// 五个写动词各配一项只读复核（看关系账的三个：`im_contacts_request` /
    /// `im_answer_contact` / `im_end_contact`；看黑名单的两个：`im_block` / `im_unblock`）——
    /// 装配期的自洽检查会盯住这件事（写效应必配复核）。
    ///
    /// - `im_contacts_request`：**同参数第二次不产生新的一段**（一对之间只有一段关系，
    ///   再申请回到那一段 ⇒ 成功、零新事实），与 `im_mark_read` 同形状 ⇒ 同一先例归
    ///   `repeatable-write`。
    /// - `im_answer_contact`：**一次决定**——接受之后再接受是幂等成功（零新事实），拒绝之后
    ///   那段已闭合（第二次得到 `unknown-contact`）。同一先例。
    /// - `im_block` / `im_unblock`：**同参数第二次是成功 no-op**（幂等 204，零新事实）⇒ 同上。
    /// - `im_end_contact`：**同参数第二次是成功 no-op**（没有 active 段也是幂等 204，零新事实）
    ///   ⇒ 同上；复核是关系账（那一段还在不在）。
    fn declared_effects(&self) -> Vec<crate::app_contract::EffectDeclaration> {
        use crate::app_contract::{EffectClass, EffectDeclaration};
        vec![
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_CONTACTS_REQUEST),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_CONTACTS)),
            ),
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_ANSWER_CONTACT),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_CONTACTS)),
            ),
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_BLOCK),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_BLOCKS)),
            ),
            // 解除关系（2026-10-13）：**同参数第二次是成功 no-op**（没有 active 段 ⇒ 幂等 204，
            // 零新事实）⇒ 与发起/应答/拉黑同一档。**复核 = 关系账**（那一段还在不在）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_END_CONTACT),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_CONTACTS)),
            ),
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_UNBLOCK),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_BLOCKS)),
            ),
        ]
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// 两个读动词带回的是**世界上别人与我之间的关系事实**，而且每一段都能指回一个明确的
    /// 对端，因此是 [`crate::app_contract::SourceClass::WorldSigned`]；五个写动词只回报服务端
    /// 对这次动作的确认（关系状态 + 对端身份是**世界记下的事实**，不是别人对我说的话）⇒
    /// [`crate::app_contract::SourceClass::System`]。
    fn declared_sources(&self) -> Vec<crate::app_contract::SourceDeclaration> {
        use crate::app_contract::{SourceClass, SourceDeclaration};
        self.declared_capabilities()
            .items()
            .iter()
            .map(|capability| {
                let class = match capability.id().as_str() {
                    CAPABILITY_CONTACTS_REQUEST
                    | CAPABILITY_ANSWER_CONTACT
                    | CAPABILITY_BLOCK
                    | CAPABILITY_UNBLOCK
                    | CAPABILITY_END_CONTACT => SourceClass::System,
                    _ => SourceClass::WorldSigned,
                };
                SourceDeclaration::new(capability.id().clone(), class)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 参数解析（工具面先拒绝 ⇒ 非法值一次都不出网）
// ---------------------------------------------------------------------------

/// 关系面的对端主体校验：非空且 ≤ [`MAX_ID_BYTES`] 字节（标识那一族的上限在 `im_channel`
/// 只有一处——房间、参与者、消息目标共用它；非法值一次都不出网；**存在性**归服务端裁决，本层不猜）。
fn valid_participant(params: &ActionParams) -> Option<&str> {
    params
        .get(PARAM_PARTICIPANT)
        .filter(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
}

/// 回答的决定词：**白名单逐字**（`accept` | `decline`）——别的一律参数非法。
fn valid_decision(params: &ActionParams) -> Option<&str> {
    params
        .get(PARAM_DECISION)
        .filter(|decision| *decision == DECISION_ACCEPT || *decision == DECISION_DECLINE)
}

/// 回答关系申请的两个决定词（词表在本层封闭）。
const DECISION_ACCEPT: &str = "accept";
const DECISION_DECLINE: &str = "decline";

// ---------------------------------------------------------------------------
// 关系面 / 拉黑面的回执渲染（批次三第三刀 第②步）
// ---------------------------------------------------------------------------

/// 关系账：`contacts = N, incoming = M` + 逐条 `peer`/`kind`/`name`/`status`/`direction`。
///
/// **`incoming` 单独计数**：那是"等**我**回答"的条数——主体醒来先看这一行，就知道有没有
/// 事情等它决定（这是把"哪条轮到我"从试错变成读数）。
fn render_contacts(body: &serde_json::Value) -> Result<String, ImFailure> {
    let contacts = body
        .get("contacts")
        .and_then(|contacts| contacts.as_array())
        .ok_or(ImFailure::Unreachable)?;
    let incoming = contacts
        .iter()
        .filter(|c| c.get("direction").and_then(|d| d.as_str()) == Some("incoming"))
        .count();
    let mut text = format!("contacts = {}, incoming = {}\n", contacts.len(), incoming);
    for contact in contacts {
        let field = |name: &str| {
            contact
                .get(name)
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        };
        let peer = contact.get("peer");
        let peer_field = |name: &str| {
            peer.and_then(|p| p.get(name))
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        };
        text.push_str(&format!(
            "peer = {} · {} · {} · {} · direction = {}\n",
            peer_field("id"),
            peer_field("kind"),
            peer_field("display_name"),
            field("status"),
            field("direction"),
        ));
    }
    Ok(text)
}

/// 拉黑名单：`blocks = N` + 逐条 `peer`/`kind`/`name`/`since`。
fn render_blocks(body: &serde_json::Value) -> Result<String, ImFailure> {
    let blocks = body
        .get("blocks")
        .and_then(|blocks| blocks.as_array())
        .ok_or(ImFailure::Unreachable)?;
    let mut text = format!("blocks = {}\n", blocks.len());
    for block in blocks {
        let field = |name: &str| {
            block
                .get(name)
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        };
        let peer = block.get("peer");
        let peer_field = |name: &str| {
            peer.and_then(|p| p.get(name))
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        };
        text.push_str(&format!(
            "peer = {} · {} · {} · since = {}\n",
            peer_field("id"),
            peer_field("kind"),
            peer_field("display_name"),
            // 关系的起点是**世界给的时刻** ⇒ 摆给主体时换成本机刻度（见 `localize_rfc3339`）。
            localize_rfc3339(field("created_at")),
        ));
    }
    Ok(text)
}

/// 从申请回执里取出对端（`contact.peer`）：拿不到就**不猜**（调用方回 `unknown`）。
fn render_peer(body: &serde_json::Value, _label: &str) -> Option<String> {
    let peer = body.get("contact")?.get("peer")?;
    let field = |name: &str| peer.get(name).and_then(|value| value.as_str());
    Some(format!(
        "{} · {} · {}",
        field("id")?,
        field("kind")?,
        field("display_name").unwrap_or("-")
    ))
}

fn render_contact_requested(peer: &str) -> String {
    format!("peer = {peer}\nrelation = pending（等他回答；`im_contacts` 看得到它的方向与状态）")
}

fn render_answered(body: &serde_json::Value, peer: &str, decision: &str) -> String {
    let status = body
        .get("contact")
        .and_then(|contact| contact.get("status"))
        .and_then(|value| value.as_str())
        .unwrap_or("-");
    format!("peer = {peer}\ndecision = {decision}\nrelation = {status}")
}

fn render_answered_declined(peer: &str) -> String {
    format!("peer = {peer}\ndecision = decline\nrelation = declined（那段已闭合，不占位）")
}

fn render_ended(peer: &str) -> String {
    format!("peer = {peer}\nended = yes（单方面生效、不通知对方；旧房永久封存。拉黑是另一条）")
}

fn render_blocked(peer: &str) -> String {
    format!("peer = {peer}\nblocked = yes（他之后向你申请只会得到「找不到这位主体」）")
}

fn render_unblocked(peer: &str) -> String {
    format!("peer = {peer}\nunblocked = yes（他可以重新申请；关系不会自动恢复）")
}
