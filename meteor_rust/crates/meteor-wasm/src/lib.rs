

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub async fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    meteor_app::wasm_assets::bootstrap().await.map_err(|e| JsValue::from_str(&e))?;
    meteor_app::build_app().run();
    Ok(())
}
