use super::*;

#[test]
fn automatic_session_names_use_the_first_prompt() {
    assert!(is_automatic_session_name("Runtime Session"));
    assert!(is_automatic_session_name("New Session"));
    assert!(!is_automatic_session_name("Release planning"));
    assert_eq!(
        automatic_session_title("  Review   the project architecture and risks  "),
        "Review the project architecture and risks"
    );
    assert_eq!(
        automatic_session_title("请帮我修复 session 自动命名"),
        "修复 session 自动命名"
    );
    assert_eq!(
        automatic_session_title("请帮我分析 MBTI，重点区分 N/S？"),
        "分析 MBTI 重点区分 N/S"
    );
    assert_eq!(
        automatic_session_title("用脑图表示一下 transformer 的原理"),
        "Transformer 原理思维导图"
    );
    assert_eq!(
        automatic_session_title("用 mindmap 描述下 transformer 架构"),
        "Transformer 架构思维导图"
    );
}

#[test]
fn semantic_session_titles_reject_raw_conversation_sentences() {
    let turns = vec![
        SessionTitleTurn {
            prompt: "你帮我画一个超时空要塞的三段变形机器人".to_string(),
            answer: "我会生成一张三段变形机器人设定图。".to_string(),
        },
        SessionTitleTurn {
            prompt: "这是高达，不是马克罗士，你重新画".to_string(),
            answer: "我会按超时空要塞 VF-1 的特征重新绘制。".to_string(),
        },
    ];

    assert!(generated_session_title_copies_conversation(
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
    assert_eq!(
        validated_generated_session_title("这是高达 不是马克罗士 你重新画", &turns),
        None
    );
    assert_eq!(
        validated_generated_session_title("重绘超时空要塞变形机器人", &turns),
        Some("重绘超时空要塞变形机器人".to_string())
    );
}

#[test]
fn generated_session_titles_never_copy_even_concise_user_topics() {
    let turns = vec![SessionTitleTurn {
        prompt: "Rust agent loop review".to_string(),
        answer: "I found two lifecycle races.".to_string(),
    }];

    assert!(generated_session_title_copies_conversation(
        "Rust agent loop review",
        &turns
    ));
    assert_eq!(
        validated_generated_session_title("Rust agent loop review", &turns),
        None
    );
    assert_eq!(
        fallback_session_title(&turns),
        Some("Rust agent loop review Overview".to_string())
    );
}

#[test]
fn session_title_fallback_recovers_a_pending_conversation_without_copying_a_request() {
    let title_like_turns = vec![SessionTitleTurn {
        prompt: "用 mindmap 描述下 transformer 架构".to_string(),
        answer: "下面用思维导图展示 Transformer 的结构。".to_string(),
    }];
    assert_eq!(
        fallback_session_title(&title_like_turns),
        Some("Transformer 架构思维导图".to_string())
    );

    let request_turns = vec![SessionTitleTurn {
        prompt: "请帮我修复 session 自动命名".to_string(),
        answer: "我会检查标题生成与持久化链路。".to_string(),
    }];
    assert_eq!(
        fallback_session_title(&request_turns),
        Some("修复 session 自动命名".to_string())
    );
}

#[test]
fn session_title_state_retries_pending_and_repairs_legacy_prompt_copies() {
    let turns = vec![SessionTitleTurn {
        prompt: "这是高达，不是马克罗士，你重新画".to_string(),
        answer: "我会按超时空要塞的设定重新绘制。".to_string(),
    }];

    assert!(session_title_refinement_needed(
        SessionTitleState::Pending,
        "New Session",
        &turns
    ));
    assert!(session_title_refinement_needed(
        SessionTitleState::Automatic,
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
    let copied_diagram_turns = vec![SessionTitleTurn {
        prompt: "用脑图表示一下 transformer 的原理".to_string(),
        answer: "下面用思维导图介绍 Transformer 原理。".to_string(),
    }];
    assert!(session_title_refinement_needed(
        SessionTitleState::Automatic,
        "用脑图表示一下 transformer 的原理",
        &copied_diagram_turns
    ));
    assert!(!session_title_refinement_needed(
        SessionTitleState::Automatic,
        "重绘超时空要塞变形机器人",
        &turns
    ));
    assert!(!session_title_refinement_needed(
        SessionTitleState::Manual,
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
}

#[test]
fn generated_session_titles_are_clean_and_bounded() {
    assert_eq!(
        cleaned_generated_session_title("**标题：桌面宠物开发。**\nextra"),
        Some("桌面宠物开发".to_string())
    );
    assert_eq!(
        cleaned_generated_session_title("Title: Review repository architecture"),
        Some("Review repository architecture".to_string())
    );
    assert_eq!(cleaned_generated_session_title("New Session"), None);
    assert_eq!(cleaned_generated_session_title("你好，Dale！我是"), None);
    assert_eq!(cleaned_generated_session_title("我是 Cindx"), None);
}

#[test]
fn session_title_context_skips_greetings_and_uses_two_meaningful_turns() {
    let message = |sequence, role: &str, content: &str| ChatMessageView {
        sequence,
        role: role.to_string(),
        content: content.to_string(),
        timestamp_ms: sequence,
        run_id: None,
        queue_id: None,
        attachments: Vec::new(),
    };
    let messages = vec![
        message(1, "user", "你好"),
        message(2, "assistant", "你好，Dale！我是 Cindx。"),
        message(3, "user", "分析我们对话里体现出的 MBTI 倾向"),
        message(4, "assistant", "我会根据具体措辞分析倾向。"),
        message(5, "user", "重点区分 N/S，并给出直接证据"),
        message(6, "assistant", "N/S 的证据主要来自抽象与细节偏好。"),
    ];

    let turns = meaningful_session_title_turns(&messages);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].prompt, "分析我们对话里体现出的 MBTI 倾向");
    assert_eq!(turns[1].prompt, "重点区分 N/S，并给出直接证据");
    assert_eq!(turns[1].answer, "N/S 的证据主要来自抽象与细节偏好。");
}

#[test]
fn greeting_only_turns_do_not_claim_a_session_title() {
    for greeting in ["你好", "您好！", "hello", "Hi there"] {
        assert!(!is_meaningful_session_title_prompt(greeting));
    }
    assert!(is_meaningful_session_title_prompt(
        "你好，帮我审查 Rust agent loop"
    ));
    assert!(is_meaningful_session_title_prompt(
        "Hello World app architecture"
    ));
}

#[test]
fn generated_session_titles_do_not_overwrite_later_edits() {
    assert!(can_apply_generated_session_title(
        "Initial request title",
        42,
        "Initial request title",
        42
    ));
    assert!(!can_apply_generated_session_title(
        "My custom title",
        43,
        "Initial request title",
        42
    ));
    assert!(!can_apply_generated_session_title(
        "Initial request title",
        43,
        "Initial request title",
        42
    ));
}
