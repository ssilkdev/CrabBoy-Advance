#![cfg(feature = "desktop")]
//! Mission + chat + guide-retrieval behaviour of the AI agent.
//!
//! These exercise the parts that run on the UI thread and need no model
//! server: instruction handling, mission lifecycle, guide indexing and the
//! plan-parsing that turns a model reply into a mission update.

use gba_simulator::ui::ai_agent::{parse_plan, AiAgent, ChatRole, MissionState};
use gba_simulator::ui::game_guide::GuideKind;

const WALKTHROUGH: &str = "\
POKEMON EMERALD - COMPLETE WALKTHROUGH

SECTION 1: LITTLEROOT TOWN
Choose your starter from Professor Birch's bag on Route 101.
Treecko, Torchic and Mudkip are the options.

SECTION 2: PETALBURG AND ROUTE 104
Head north through Route 102 and 103. Catch a Zigzagoon for Pickup.
Petalburg Gym is run by Norman but he will not battle you yet.

SECTION 3: RUSTBORO CITY GYM - THE FIRST BADGE
The first gym badge in Hoenn is the Stone Badge, held by Leader Roxanne.
Roxanne uses Rock-type Pokemon: Geodude level 12, Geodude level 12 and
Nosepass level 15. Nosepass knows Rock Tomb and Block.
Bring a Grass or Water Pokemon. Marshtomp with Mud Shot sweeps the entire gym.
The gym is in the north-west of Rustboro City, a grey building with a rock sign.
Two Youngster trainers stand between the door and Roxanne; defeat both first.
Beating Roxanne awards the Stone Badge and TM39 Rock Tomb.

SECTION 4: ROUTE 116 AND RUSTURF TUNNEL
After earning the badge, chase the Team Aqua grunt who stole the Devon Goods.
Follow him east out of Rustboro into Route 116 and then into Rusturf Tunnel.

SECTION 5: DEWFORD TOWN AND THE SECOND BADGE
Sail with Mr Briney to Dewford. Brawly holds the Knuckle Badge and uses
Fighting types such as Machop and Makuhita. Flying and Psychic moves win here.
";

fn agent_with_guide() -> AiAgent {
    let mut agent = AiAgent::new();
    agent
        .guides
        .add_text("emerald_walkthrough.txt".into(), GuideKind::Text, WALKTHROUGH)
        .expect("index walkthrough");
    agent
}

#[test]
fn instruction_becomes_the_active_mission() {
    let mut agent = AiAgent::new();
    assert!(agent.mission.is_none());

    agent.submit_instruction("Complete the first trainer badge", 1000);

    let m = agent.mission.as_ref().expect("mission created");
    assert_eq!(m.text, "Complete the first trainer badge");
    assert_eq!(m.state, MissionState::Active);
    assert_eq!(m.started_frame, 1000);

    // The user's words and an acceptance notice both land in the chat.
    assert!(agent
        .chat
        .iter()
        .any(|c| c.role == ChatRole::User && c.text == "Complete the first trainer badge"));
    assert!(agent
        .chat
        .iter()
        .any(|c| c.role == ChatRole::System && c.text.contains("Mission accepted")));
}

#[test]
fn blank_instruction_is_ignored() {
    let mut agent = AiAgent::new();
    agent.submit_instruction("   \n  ", 0);
    assert!(agent.mission.is_none());
    assert!(agent.chat.is_empty());
}

#[test]
fn new_instruction_supersedes_the_previous_mission() {
    let mut agent = AiAgent::new();
    agent.submit_instruction("Complete the first trainer badge", 10);
    agent.submit_instruction("Catch a Ralts on Route 102", 500);

    assert_eq!(
        agent.mission.as_ref().unwrap().text,
        "Catch a Ralts on Route 102"
    );
    assert_eq!(agent.mission_history.len(), 1);
    assert_eq!(agent.mission_history[0].state, MissionState::Cancelled);
    assert_eq!(
        agent.mission_history[0].text,
        "Complete the first trainer badge"
    );
}

#[test]
fn cancelling_a_mission_retires_it() {
    let mut agent = AiAgent::new();
    agent.submit_instruction("Complete the first trainer badge", 10);
    agent.cancel_mission(99);

    assert!(agent.mission.is_none());
    assert_eq!(agent.mission_history.len(), 1);
    assert_eq!(agent.mission_history[0].state, MissionState::Cancelled);
}

#[test]
fn guide_retrieval_answers_the_badge_mission() {
    let agent = agent_with_guide();

    // The user's phrasing never says "Roxanne" or "Rustboro" — retrieval has
    // to bridge "first trainer badge" to the right walkthrough section.
    let hits = agent.guides.retrieve("Complete the first trainer badge", 3);
    assert!(!hits.is_empty(), "no excerpts retrieved");
    let top = &hits[0].text.to_lowercase();
    assert!(
        top.contains("roxanne") && top.contains("stone badge"),
        "wrong section retrieved: {}",
        hits[0].text
    );
}

#[test]
fn guide_retrieval_tracks_progress_to_a_later_section() {
    let agent = agent_with_guide();

    // Once the badge is won, a query reflecting the new situation must move
    // on to the following section rather than re-serving the gym.
    let hits = agent
        .guides
        .retrieve("Stone Badge obtained, Team Aqua grunt stole Devon Goods, chase him", 3);
    assert!(!hits.is_empty());
    let joined = hits
        .iter()
        .map(|h| h.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("rusturf") || joined.contains("route 116"),
        "expected the post-badge section, got: {}",
        joined
    );
}

#[test]
fn guide_context_block_is_cited_and_bounded() {
    let mut agent = agent_with_guide();
    let block = agent
        .guides
        .context_block("first trainer badge Roxanne gym", 3, 2500)
        .expect("context block produced");

    assert!(block.contains("GAME GUIDE EXCERPTS"));
    assert!(block.to_lowercase().contains("roxanne"));
    assert!(block.len() <= 2600, "block was {} chars", block.len());
    assert!(!agent.guides.last_citations.is_empty());
    assert!(agent.guides.last_citations[0].contains("emerald_walkthrough.txt"));
}

#[test]
fn model_reply_carries_mission_progress_and_completion() {
    let reply = r#"{
        "observation": "Roxanne has fainted and a badge animation is playing.",
        "goal": "Confirm the badge and leave the gym.",
        "say": "Got the Stone Badge!",
        "mission_progress": "Defeated Roxanne; Stone Badge awarded.",
        "mission_complete": true,
        "actions": [{"buttons": ["A"], "frames": 4}, {"buttons": [], "frames": 30}]
    }"#;
    let plan = parse_plan(reply).expect("plan parses");

    assert_eq!(plan.say, "Got the Stone Badge!");
    assert_eq!(plan.mission_progress, "Defeated Roxanne; Stone Badge awarded.");
    assert!(plan.mission_complete);
    assert_eq!(plan.actions.len(), 2);
}

#[test]
fn mission_completion_string_forms_are_accepted() {
    // Models frequently emit a string where the schema asks for a bool.
    for val in ["true", "yes", "done", "completed"] {
        let reply = format!(
            r#"{{"observation":"x","goal":"y","mission_complete":"{}",
                 "actions":[{{"buttons":["A"],"frames":4}}]}}"#,
            val
        );
        let plan = parse_plan(&reply).expect("plan parses");
        assert!(plan.mission_complete, "{:?} should mean complete", val);
    }
    for val in ["false", "no", "not yet", ""] {
        let reply = format!(
            r#"{{"observation":"x","goal":"y","mission_complete":"{}",
                 "actions":[{{"buttons":["A"],"frames":4}}]}}"#,
            val
        );
        let plan = parse_plan(&reply).expect("plan parses");
        assert!(!plan.mission_complete, "{:?} should NOT mean complete", val);
    }
}

#[test]
fn plan_without_mission_fields_still_parses() {
    // Backwards compatibility: the pre-mission prompt shape must keep working.
    let reply = r#"{"observation":"Title screen","goal":"Start the game",
                    "actions":[{"buttons":["START"],"frames":4}]}"#;
    let plan = parse_plan(reply).expect("plan parses");
    assert!(plan.say.is_empty());
    assert!(!plan.mission_complete);
    assert_eq!(plan.actions.len(), 1);
}

#[test]
fn agent_learns_from_an_uploaded_pdf_end_to_end() {
    // The real user path: a PDF file goes in via load_guide, and afterwards
    // the agent can retrieve from it. Uses the shipped manual as the fixture.
    let path = std::path::Path::new("CrabBoy_Advance_User_Manual.pdf");
    if !path.exists() {
        eprintln!("manual PDF fixture absent; skipping");
        return;
    }
    let mut agent = AiAgent::new();
    let msg = agent.load_guide(path, 4242).expect("load pdf guide");

    assert!(msg.contains("Learned guide"), "unexpected notice: {}", msg);
    assert_eq!(agent.guides.docs.len(), 1);
    assert_eq!(agent.guides.docs[0].kind, GuideKind::Pdf);
    assert!(agent.guides.docs[0].chunks > 1);

    // The import is announced in the chat so the user sees it happened.
    assert!(agent
        .chat
        .iter()
        .any(|c| c.role == ChatRole::System && c.text.contains("Learned guide")));

    // And the content is actually queryable.
    let hits = agent.guides.retrieve("save states and rewind", 3);
    assert!(!hits.is_empty(), "nothing retrievable from the imported PDF");
}

#[test]
fn rejecting_a_bad_guide_does_not_poison_the_library() {
    let dir = std::env::temp_dir().join("crabboy_guide_reject_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("tiny.txt");
    std::fs::write(&bad, "nope").unwrap();

    let mut agent = AiAgent::new();
    assert!(agent.load_guide(&bad, 0).is_err());
    assert!(agent.guides.docs.is_empty());
    assert!(agent.guides.is_empty());

    let _ = std::fs::remove_file(&bad);
}

#[test]
fn guide_library_reports_what_it_learned() {
    let agent = agent_with_guide();
    assert_eq!(agent.guides.docs.len(), 1);
    let doc = &agent.guides.docs[0];
    assert_eq!(doc.kind, GuideKind::Text);
    assert!(doc.chunks >= 1);
    assert!(doc.chars > 500);
    assert!(!agent.guides.is_empty());
}

#[test]
fn disabling_every_guide_makes_the_library_empty_for_retrieval() {
    let mut agent = agent_with_guide();
    agent.guides.docs[0].enabled = false;
    assert!(agent.guides.is_empty());
    assert!(agent.guides.retrieve("Roxanne", 3).is_empty());
}
