use validator::Validate;

#[derive(Debug, Clone, Validate)]
pub struct CreateNodeRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(length(min = 1, max = 255))]
    pub host: String,

    #[validate(range(min = 1, max = 65535))]
    pub port: i32,

    #[validate(length(min = 1, max = 20))]
    pub protocol: String,

    #[validate(length(max = 500))]
    pub description: Option<String>,

    #[validate(range(min = 1, max = 10000))]
    pub max_connections: i32,

    pub allow_relay: bool,

    #[validate(length(min = 1, max = 100))]
    pub network_name: String,

    #[validate(length(max = 100))]
    pub network_secret: Option<String>,

    #[validate(length(max = 20))]
    pub qq_number: Option<String>,

    #[validate(length(max = 50))]
    pub wechat: Option<String>,

    #[validate(email, length(max = 100))]
    pub mail: Option<String>,
}
