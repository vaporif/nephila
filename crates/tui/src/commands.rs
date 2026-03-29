#[derive(Debug, PartialEq)]
pub enum Command {
    Spawn {
        objective: String,
        dir: Option<String>,
    },
    Kill {
        agent: String,
    },
    Pause {
        agent: String,
    },
    Resume {
        agent: String,
    },
    Rollback {
        agent: String,
        version: u32,
    },
    Respond {
        agent: String,
        choice: String,
    },
    Inject {
        agent: String,
        message: String,
    },
    Tree,
    Filter {
        target: String,
    },
    Help,
    Quit,
}

pub fn parse(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("empty command".into());
    }

    let mut parts = input.splitn(3, ' ');
    let cmd = parts.next().unwrap();
    let arg1 = parts.next();
    let arg2 = parts.next();

    match cmd {
        "spawn" => {
            let objective = arg1
                .ok_or_else(|| "spawn requires an objective".to_string())?
                .to_string();
            let dir = arg2.map(|s| s.to_string());
            Ok(Command::Spawn { objective, dir })
        }
        "kill" => {
            let agent = arg1
                .ok_or_else(|| "kill requires an agent id".to_string())?
                .to_string();
            Ok(Command::Kill { agent })
        }
        "pause" => {
            let agent = arg1
                .ok_or_else(|| "pause requires an agent id".to_string())?
                .to_string();
            Ok(Command::Pause { agent })
        }
        "resume" => {
            let agent = arg1
                .ok_or_else(|| "resume requires an agent id".to_string())?
                .to_string();
            Ok(Command::Resume { agent })
        }
        "rollback" => {
            let agent = arg1
                .ok_or_else(|| "rollback requires an agent id".to_string())?
                .to_string();
            let version_str =
                arg2.ok_or_else(|| "rollback requires a version number".to_string())?;
            let version = version_str
                .parse::<u32>()
                .map_err(|_| format!("invalid version number: {version_str}"))?;
            Ok(Command::Rollback { agent, version })
        }
        "respond" => {
            let agent = arg1
                .ok_or_else(|| "respond requires an agent id".to_string())?
                .to_string();
            let choice = arg2
                .ok_or_else(|| "respond requires a choice".to_string())?
                .to_string();
            Ok(Command::Respond { agent, choice })
        }
        "inject" => {
            let agent = arg1
                .ok_or_else(|| "inject requires an agent id".to_string())?
                .to_string();
            let message = arg2
                .ok_or_else(|| "inject requires a message".to_string())?
                .to_string();
            Ok(Command::Inject { agent, message })
        }
        "tree" => Ok(Command::Tree),
        "filter" => {
            let target = arg1
                .ok_or_else(|| "filter requires a target".to_string())?
                .to_string();
            Ok(Command::Filter { target })
        }
        "help" => Ok(Command::Help),
        "quit" | "q" => Ok(Command::Quit),
        _ => Err(format!("unknown command: {cmd}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kill() {
        assert_eq!(
            parse("kill abc123").unwrap(),
            Command::Kill {
                agent: "abc123".into()
            }
        );
    }

    #[test]
    fn parse_kill_missing_arg() {
        assert!(parse("kill").is_err());
    }

    #[test]
    fn parse_rollback() {
        assert_eq!(
            parse("rollback agent1 3").unwrap(),
            Command::Rollback {
                agent: "agent1".into(),
                version: 3
            }
        );
    }

    #[test]
    fn parse_rollback_bad_version() {
        assert!(parse("rollback agent1 abc").is_err());
    }

    #[test]
    fn parse_respond() {
        assert_eq!(
            parse("respond agent1 yes please").unwrap(),
            Command::Respond {
                agent: "agent1".into(),
                choice: "yes please".into()
            }
        );
    }

    #[test]
    fn parse_quit() {
        assert_eq!(parse("quit").unwrap(), Command::Quit);
        assert_eq!(parse("q").unwrap(), Command::Quit);
    }

    #[test]
    fn parse_unknown() {
        assert!(parse("foobar").is_err());
    }

    #[test]
    fn parse_empty() {
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn parse_spawn_with_dir() {
        assert_eq!(
            parse("spawn build-ui /tmp/project").unwrap(),
            Command::Spawn {
                objective: "build-ui".into(),
                dir: Some("/tmp/project".into())
            }
        );
    }

    #[test]
    fn parse_spawn_without_dir() {
        assert_eq!(
            parse("spawn build-ui").unwrap(),
            Command::Spawn {
                objective: "build-ui".into(),
                dir: None
            }
        );
    }

    #[test]
    fn parse_tree() {
        assert_eq!(parse("tree").unwrap(), Command::Tree);
    }

    #[test]
    fn parse_help() {
        assert_eq!(parse("help").unwrap(), Command::Help);
    }

    #[test]
    fn parse_inject() {
        assert_eq!(
            parse("inject agent1 focus on the API layer").unwrap(),
            Command::Inject {
                agent: "agent1".into(),
                message: "focus on the API layer".into()
            }
        );
    }

    #[test]
    fn parse_filter() {
        assert_eq!(
            parse("filter agent1").unwrap(),
            Command::Filter {
                target: "agent1".into()
            }
        );
    }

    #[test]
    fn parse_pause_resume() {
        assert_eq!(
            parse("pause agent1").unwrap(),
            Command::Pause {
                agent: "agent1".into()
            }
        );
        assert_eq!(
            parse("resume agent1").unwrap(),
            Command::Resume {
                agent: "agent1".into()
            }
        );
    }
}
