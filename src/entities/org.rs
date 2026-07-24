use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "org")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub slug: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::package::Entity")]
    Package,
}

impl Related<super::package::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Package.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
