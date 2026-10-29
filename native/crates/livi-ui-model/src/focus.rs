//! What a UI plugin's components can take the focus on, for keys and a rotary knob (docs/plugins.md, "Keys"): a
//! list per page, by its path, and one for the plugin's side bar where it takes the place of LIVI's navigation. The
//! plugin declares them with `Livi.action("focus-targets", ...)`; LIVI's focus model (keys.rs) moves over them.

/// The scope of the list for the plugin's side bar
pub const NAV: &str = "nav";

/// The most targets one list may have
pub const MAX_TARGETS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    /// left and right, and the knob while it is being edited, change it instead of moving on ("slider")
    pub slider: bool,
    /// in the side bar: the button of the page on screen, where the focus lands coming back from the page
    /// ("current")
    pub current: bool,
}

/// A declaration: `<scope> [<id>[:<flag>]... ...]`. The scope is `nav` or a page's path (`/plugin/<id>`, `/camera`
/// ...); an id is any text without spaces and colons; the flags are `slider` and `current`.
pub fn parse(argument: &str) -> Result<(String, Vec<Target>), String> {
    let mut words = argument.split_whitespace();
    let scope = words.next().ok_or("no scope")?;
    if scope != NAV && !scope.starts_with('/') {
        return Err(format!("scope {scope} is neither \"{NAV}\" nor a path"));
    }
    let mut targets: Vec<Target> = Vec::new();
    for word in words {
        let mut parts = word.split(':');
        let id = parts.next().unwrap_or_default();
        if id.is_empty() {
            return Err(format!("{word} has no id"));
        }
        if targets.iter().any(|t| t.id == id) {
            return Err(format!("{id} is there twice"));
        }
        let mut target = Target {
            id: id.to_string(),
            slider: false,
            current: false,
        };
        for flag in parts {
            match flag {
                "slider" => target.slider = true,
                "current" => target.current = true,
                _ => return Err(format!("{id} has an unknown flag {flag}")),
            }
        }
        targets.push(target);
    }
    if targets.len() > MAX_TARGETS {
        return Err(format!("more than {MAX_TARGETS} targets"));
    }
    Ok((scope.to_string(), targets))
}

/// The lists the plugin declared, by scope
#[derive(Debug, Default)]
pub struct Targets {
    lists: Vec<(String, Vec<Target>)>,
}

impl Targets {
    pub const fn new() -> Self {
        Self { lists: Vec::new() }
    }

    /// Takes a declaration (parse): its list replaces the one the scope had, an empty one takes it away
    pub fn declare(&mut self, argument: &str) -> Result<(), String> {
        let (scope, targets) = parse(argument)?;
        self.lists.retain(|(s, _)| *s != scope);
        if !targets.is_empty() {
            self.lists.push((scope, targets));
        }
        Ok(())
    }

    /// The targets of a page, by its path; none when the plugin declared none
    pub fn page(&self, path: &str) -> &[Target] {
        self.list(path)
    }

    /// The targets of the plugin's side bar
    pub fn nav(&self) -> &[Target] {
        self.list(NAV)
    }

    fn list(&self, scope: &str) -> &[Target] {
        self.lists
            .iter()
            .find(|(s, _)| s == scope)
            .map(|(_, t)| t.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(targets: &[Target]) -> Vec<&str> {
        targets.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn a_page_declares_its_targets_in_order() {
        let mut t = Targets::new();
        t.declare("/plugin/home weather media.play volume:slider")
            .unwrap();
        assert_eq!(
            ids(t.page("/plugin/home")),
            vec!["weather", "media.play", "volume"]
        );
        assert!(t.page("/plugin/home")[2].slider);
        assert!(!t.page("/plugin/home")[0].slider);
        assert!(t.page("/plugin/media").is_empty());
        assert!(t.nav().is_empty());
    }

    #[test]
    fn the_side_bar_marks_the_page_on_screen() {
        let mut t = Targets::new();
        t.declare("nav home:current maps media").unwrap();
        assert_eq!(ids(t.nav()), vec!["home", "maps", "media"]);
        assert!(t.nav()[0].current);
        assert!(!t.nav()[1].current);
        let both = parse("nav dimmer:slider:current").unwrap().1;
        assert!(both[0].slider && both[0].current);
    }

    #[test]
    fn a_new_list_replaces_the_old_and_an_empty_one_takes_it_away() {
        let mut t = Targets::new();
        t.declare("/camera a b").unwrap();
        t.declare("/camera c").unwrap();
        assert_eq!(ids(t.page("/camera")), vec!["c"]);
        t.declare("  /camera  ").unwrap();
        assert!(t.page("/camera").is_empty());
    }

    #[test]
    fn broken_declarations_are_refused_and_change_nothing() {
        let mut t = Targets::new();
        t.declare("/plugin/a x").unwrap();
        for bad in [
            "",
            "plugin/a x",
            "/plugin/a :slider",
            "/plugin/a x x",
            "/plugin/a x:bold",
        ] {
            assert!(t.declare(bad).is_err(), "{bad}");
        }
        assert_eq!(ids(t.page("/plugin/a")), vec!["x"]);
        let many = (0..=MAX_TARGETS)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(t.declare(&format!("/plugin/a {many}")).is_err());
    }
}
