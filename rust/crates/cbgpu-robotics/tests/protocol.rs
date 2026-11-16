use cbgpu_robotics::{planning, protocol as p, remote_protocol as r};
use serde_json::{Value, json};
fn caps() -> Value {
    json!([{"name":"navigate_to","description":"Navigate to a known waypoint","parameters":{"type":"object","properties":{"waypoint":{"type":"string","maxLength":32,"enum":["desk","charging_station"]}},"required":["waypoint"],"additionalProperties":false},"max_duration_s":10}])
}
fn plan() -> Value {
    json!({"summary":"Move to charging station","needs_clarification":false,"question":"","steps":[{"capability":"navigate_to","arguments":{"waypoint":"charging_station"},"duration_s":8}]})
}
fn map() -> Value {
    json!({"width":4,"height":4,"resolution":0.5,"origin":{"x":0,"y":0,"yaw":0},"frame":"map","session":"mapping-1","version":1,"rle":[0,8,-1,4,100,4]})
}
#[test]
fn capability_digest_is_byte_compatible_with_python_including_unicode_and_scientific_floats() {
    assert_eq!(
        p::digest(&caps()).unwrap(),
        "1a4a1fa976068699a77b849af19c09c6288b29a5f0ec9b43a3eccfca82bd6d78"
    );
    let floats = json!([{"name":"set_speed","description":"속도 설정","parameters":{"type":"object","properties":{"speed":{"type":"number","minimum":0.00001,"maximum":1e20}},"required":["speed"],"additionalProperties":false},"max_duration_s":1.0}]);
    assert_eq!(
        p::digest(&floats).unwrap(),
        "a397c62cf2cabac3258c26038de2deab35b202bd80738acc6350f182b68339d9"
    )
}
#[test]
fn unsupported_unbounded_deep_and_duplicate_capabilities_are_rejected() {
    let mut bad = caps();
    bad[0]["parameters"]["additionalProperties"] = json!(true);
    assert!(p::capabilities(&bad).is_err());
    bad = caps();
    bad[0]["parameters"]["properties"]["waypoint"]
        .as_object_mut()
        .unwrap()
        .remove("maxLength");
    assert!(p::capabilities(&bad).is_err());
    bad = caps();
    bad[0]["max_duration_s"] = json!(31);
    assert!(p::capabilities(&bad).is_err());
    let first = caps()[0].clone();
    assert!(p::capabilities(&json!([first, first])).is_err());
    assert!(p::capabilities(&json!([])).is_err());
    assert!(p::capabilities(&json!([{"name":"shell"}])).is_err());
    let mut schema = json!({"type":"boolean"});
    for _ in 0..6 {
        schema = json!({"type":"array","maxItems":1,"items":schema})
    }
    assert!(p::schema_check(&schema, 0).is_err());
}
#[test]
fn bounded_schema_types_reject_bool_as_number_unknown_arguments_and_wrong_enums() {
    let schema = json!({"type":"integer","minimum":0,"maximum":10});
    for bad in [json!(true), json!(1.0), json!(-1), json!(11)] {
        assert!(p::validate(&bad, &schema).is_err())
    }
    assert!(p::validate(&json!(2), &schema).is_ok());
    let parameters = &caps()[0]["parameters"];
    assert!(p::validate(&json!({"waypoint":"desk","shell":"rm"}), parameters).is_err());
    assert!(p::validate(&json!({"waypoint":"unknown"}), parameters).is_err());
    assert!(p::validate(&json!({}), parameters).is_err());
}
#[test]
fn proposal_requires_known_bounded_steps_or_explicit_clarification() {
    assert!(p::proposal(&plan(), &caps()).is_ok());
    for mutation in ["capability", "arguments", "duration"] {
        let mut value = plan();
        match mutation {
            "capability" => value["steps"][0]["capability"] = json!("shell"),
            "arguments" => value["steps"][0]["arguments"]["command"] = json!("anything"),
            _ => value["steps"][0]["duration_s"] = json!(999),
        }
        assert!(p::proposal(&value, &caps()).is_err())
    }
    assert!(p::proposal(&json!({"summary":"Need waypoint","needs_clarification":true,"question":"Which waypoint?","steps":[]}),&caps()).is_ok());
    let mut value = plan();
    value["steps"] = json!([]);
    assert!(p::proposal(&value, &caps()).is_err());
    value = plan();
    value["steps"] = Value::Array(vec![value["steps"][0].clone(); 7]);
    assert!(p::proposal(&value, &caps()).is_err());
}
#[test]
fn mission_envelope_checks_robot_digest_approval_and_freshness() {
    let at = 1000.;
    let mut value = json!({"protocol_version":1,"id":"11111111-1111-4111-8111-111111111111","robot_id":"car-1","capabilities_digest":p::digest(&caps()).unwrap(),"issued_at":at,"expires_at":at+90.,"observed_at":at-20.,"requires_local_approval":true,"plan":plan()});
    assert!(p::envelope(&value, &caps(), "car-1", at).is_ok());
    assert!(p::envelope(&value, &caps(), "other", at).is_err());
    assert!(p::envelope(&value, &caps(), "car-1", at + 100.).is_err());
    value["requires_local_approval"] = json!(false);
    assert!(p::envelope(&value, &caps(), "car-1", at).is_err());
}
#[test]
fn map_coordinates_reject_unknown_obstacles_outside_and_nonfinite() {
    let grid = r::grid(&map()).unwrap();
    assert_eq!(r::cell(&grid, 0.75, 0.75).unwrap(), 0);
    for (x, y) in [(0.25, 1.25), (0.25, 1.75), (100., 0.), (f64::NAN, 0.)] {
        assert!(r::free_goal(&grid, x, y).is_err())
    }
    let mut rotated = map();
    rotated["origin"]["yaw"] = json!(std::f64::consts::FRAC_PI_2);
    assert_eq!(
        r::cell(&r::grid(&rotated).unwrap(), -0.75, 0.75).unwrap(),
        0
    );
}
#[test]
fn malformed_maps_and_rle_are_bounded() {
    for (field, value) in [
        ("width", json!(513)),
        ("height", json!(0)),
        ("resolution", json!(0.001)),
        ("rle", json!([0, 15])),
        ("rle", json!([101, 16])),
        ("version", json!(0)),
    ] {
        let mut value_map = map();
        value_map[field] = value;
        assert!(r::grid(&value_map).is_err())
    }
    assert!(r::pose(&json!({"x":0,"y":0,"yaw":100001})).is_err());
}
#[test]
fn stale_sensors_missing_pose_and_invalid_flags_disable_navigation() {
    let t = json!({"armed":true,"ready":true,"emergency_stop":false,"navigation_enabled":true,"pose":{"x":0,"y":0,"yaw":0},"sensor_age_s":0,"map_session":"mapping-1","map_version":1});
    assert_eq!(r::telemetry(&t).unwrap()["ready"], true);
    let mut stale = t.clone();
    stale["sensor_age_s"] = json!(9);
    assert_eq!(r::telemetry(&stale).unwrap()["ready"], false);
    stale = t.clone();
    stale["pose"] = Value::Null;
    assert_eq!(r::telemetry(&stale).unwrap()["ready"], false);
    stale = t;
    stale["armed"] = json!(1);
    assert!(r::telemetry(&stale).is_err());
}
#[test]
fn preview_images_require_bounded_jpeg_bytes() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    assert!(r::jpeg(&STANDARD.encode([0xff, 0xd8, 0xff, 0xd9])).is_ok());
    assert!(r::jpeg("not-base64").is_err());
    assert!(r::jpeg(&STANDARD.encode(b"not JPEG")).is_err());
    assert_eq!(r::jpeg(&"x".repeat(350001)).unwrap_err().status, 413);
}
#[test]
fn planning_request_enforces_observation_freshness_models_effort_and_finite_json() {
    let at = 1000.;
    let body = json!({"instruction":"Go to charging station","robot_id":"test-robot","capabilities":caps(),"observations":{"location":"desk"},"observed_at":at});
    let (_, _, _, _, input) = planning::create_input(&body, at).unwrap();
    assert_eq!(input["max_tokens"], 2048);
    assert_eq!(
        input["tool_choice"]["function"]["name"],
        "propose_robot_plan"
    );
    for (field, value) in [
        ("observed_at", json!(at - 50.)),
        ("model", json!("unknown")),
        ("model", json!(1)),
        ("effort", json!("impossible")),
        ("instruction", json!("x".repeat(2001))),
    ] {
        let mut bad = body.clone();
        bad[field] = value;
        assert!(planning::create_input(&bad, at).is_err())
    }
}
#[test]
fn truncated_or_wrong_model_results_never_issue_a_mission() {
    let at = 1000.;
    let result = json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"propose_robot_plan","arguments":plan().to_string()}}]}}]});
    let id = "11111111-1111-4111-8111-111111111111";
    assert!(planning::validate_model_result(id, "car-1", &caps(), at, &result, at).is_ok());
    assert!(planning::validate_model_result(id, "car-1", &caps(), 0., &result, at).is_err());
    for (field, value) in [("truncated", json!(true)), ("truncated", json!("yes"))] {
        let mut bad = result.clone();
        bad[field] = value;
        assert!(planning::validate_model_result(id, "car-1", &caps(), at, &bad, at).is_err())
    }
    let mut bad = result;
    bad["choices"][0]["finish_reason"] = json!("length");
    assert!(planning::validate_model_result(id, "car-1", &caps(), at, &bad, at).is_err());
}
