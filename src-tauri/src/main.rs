// 桌面端 release 不弹控制台；Android 忽略此属性
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sonotheca_lib::run();
}
