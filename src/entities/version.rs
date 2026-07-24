use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "version")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub package_id: Uuid,
    pub version: String,
    pub sha256: String,
    pub size: i64,
    pub format: String,
    pub vcs_tag: String,
    pub vcs_commit: Option<String>,
    pub artifact_key: String,
    pub yanked: bool,
    pub published_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::package::Entity",
        from = "Column::PackageId",
        to = "super::package::Column::Id"
    )]
    Package,
}

impl Related<super::package::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Package.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
