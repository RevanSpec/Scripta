// Sous Windows, une application de bureau ne doit pas ouvrir de console :
// l'attribut ne s'applique qu'en release, pour garder les logs en débogage.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    scripta_desktop_lib::run()
}
