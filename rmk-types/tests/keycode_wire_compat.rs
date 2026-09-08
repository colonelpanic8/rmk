use rmk_types::keycode::{ConsumerKey, HidKeyCode};

#[test]
fn existing_keycodes_keep_their_wire_ids() {
    let mut buffer = [0; 8];
    // Fixtures encoded before the brightness min/max/auto variants were added.
    let bytes = [4];
    assert_eq!(postcard::from_bytes::<ConsumerKey>(&bytes).unwrap(), ConsumerKey::Play);
    assert_eq!(postcard::to_slice(&ConsumerKey::Play, &mut buffer).unwrap(), bytes);
    let bytes = [51];
    assert_eq!(
        postcard::from_bytes::<ConsumerKey>(&bytes).unwrap(),
        ConsumerKey::AcSoftKeyLeft
    );
    assert_eq!(
        postcard::to_slice(&ConsumerKey::AcSoftKeyLeft, &mut buffer).unwrap(),
        bytes
    );
    let bytes = [195, 1];
    assert_eq!(postcard::from_bytes::<HidKeyCode>(&bytes).unwrap(), HidKeyCode::MouseUp);
    assert_eq!(postcard::to_slice(&HidKeyCode::MouseUp, &mut buffer).unwrap(), bytes);
    let bytes = [214, 1];
    assert_eq!(postcard::from_bytes::<HidKeyCode>(&bytes).unwrap(), HidKeyCode::LCtrl);
    assert_eq!(postcard::to_slice(&HidKeyCode::LCtrl, &mut buffer).unwrap(), bytes);
    let bytes = [221, 1];
    assert_eq!(postcard::from_bytes::<HidKeyCode>(&bytes).unwrap(), HidKeyCode::RGui);
    assert_eq!(postcard::to_slice(&HidKeyCode::RGui, &mut buffer).unwrap(), bytes);
}
